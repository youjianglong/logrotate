use chrono::{DateTime, Local};
use clap::ValueEnum;
use fs::File;
use libflate::gzip::Encoder;
use std::cell::RefCell;
use std::fs;
use std::io;
use std::io::Error;
use std::io::{copy, ErrorKind, Write};
use std::path;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::utils;

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum CutMode {
    Size,  // Represents the mode for cutting logs based on size
    Daily, // Represents the mode for cutting logs on a daily basis
}

const DATE_FMT: &str = "%Y%m%d"; // Date format: Year-Month-Day

// Returns the current day as a string in a specific format
fn day() -> String {
    Local::now().format(DATE_FMT).to_string()
}

fn date_add(days: i64) -> String {
    let dt: DateTime<Local> = Local::now();
    let new_dt = dt + chrono::Duration::days(days);
    new_dt.format(DATE_FMT).to_string()
}

// Checks if a file exists at the given path
fn is_file(path: &String) -> bool {
    return fs::metadata(path).is_ok_and(|meta| meta.is_file());
}

fn file_glob(file_path: &String) -> io::Result<Vec<String>> {
    let p = path::Path::new(&file_path);
    let dir_path = p.parent();
    let base_path = p.file_name();
    if dir_path.is_none() {
        return Err(Error::new(ErrorKind::Other, "invalid file path"));
    }
    if base_path.is_none() {
        return Err(Error::new(ErrorKind::Other, "invalid file name"));
    }
    let dir = dir_path.unwrap();
    if !dir.exists() {
        return Err(Error::new(ErrorKind::NotFound, "directory does not exist"));
    }
    let bp = base_path.unwrap().to_str();
    if bp.is_none() {
        return Err(Error::new(ErrorKind::Other, "invalid base directory"));
    }
    let base_path: String = bp.unwrap().to_string();
    let mut files = vec![];
    for e in dir.read_dir()? {
        let de = e?;
        if let Ok(file_name) = de.file_name().into_string() {
            if file_name.starts_with(&base_path) {
                if let Some(s) = dir.join(&file_name).as_os_str().to_str() {
                    files.push(s.to_string());
                } else {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("invalid file name: {}", &file_name),
                    ));
                }
            }
        } else {
            return Err(Error::new(
                ErrorKind::Other,
                format!("invalid file name: {:?}", de.file_name()),
            ));
        }
    }
    return Ok(files);
}

// Opens a file at the given path and returns a tuple containing the file handle and its metadata
// If the file does not exist, it creates a new file and returns the file handle without metadata
// Prints an error message if there is an error opening or creating the file
fn open_file(path: &str) -> io::Result<(File, Option<fs::Metadata>)> {
    match fs::metadata(path) {
        Ok(meta) => File::options()
            .append(true)
            .open(path)
            .map(move |fp| (fp, Some(meta))),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => File::create(path).map(|fp| (fp, None)),
            _ => {
                log!("failed to read file \"{}\" metadata: {:+?}", path, err);
                Err(err)
            }
        },
    }
}

// Flushes the contents of the file handle to disk
// Prints an error message if there is an error flushing the file
fn file_flush(file: &Option<File>) {
    if let Some(mut fp) = file.as_ref() {
        if let Err(err) = fp.flush() {
            log!("failed to flush file: {:+?}", err)
        }
    }
}

fn gzip_encode(filename: &String) -> io::Result<()> {
    let mut inf = File::open(filename)?;
    let out = File::create(format!("{}.gz", filename))?;
    let mut encoder = Encoder::new(out)?;
    copy(&mut inf, &mut encoder)?;
    drop(inf);
    drop(encoder.finish().into_result()?);
    fs::remove_file(filename)?;
    Ok(())
}

fn remove_log_files(file_path: &String, day: &String) {
    let file_path = format!("{}.{}", file_path, day);
    match file_glob(&file_path) {
        Ok(files) => {
            for file in files {
                match fs::remove_file(&file) {
                    Ok(_) => {
                        log!("removed file \"{}\"", &file);
                    }
                    Err(err) => {
                        log!("failed to remove file \"{}\": {:+?}", &file, err);
                    }
                }
            }
        }
        Err(err) => {
            log!("failed to list log files: {:+?}", err);
        }
    }
}

pub trait Rotate {
    // Rotates the filename by appending the current day to it
    // If the rotated filename already exists, it appends a unique identifier to it
    fn rotate_filename(&self, path: &String, compress: bool, mul: bool) -> String {
        let day = day();
        if !mul {
            // If no multi mode, check if the file exists
            let filename = path.clone() + "." + day.as_str();
            if (compress && !is_file(&format!("{}.gz", filename)))
                || (!compress && !is_file(&filename))
            {
                return filename;
            }
        }

        let mut i = 1;
        loop {
            let filename = format!("{:}.{:}-{:}", path, day, i);
            if (compress && !is_file(&format!("{}.gz", filename)))
                || (!compress && !is_file(&filename))
            {
                return filename;
            }
            i += 1;
        }
    }

    fn receiver(&mut self) -> &mut mpsc::Receiver<Vec<u8>>;
    fn get_file(&mut self, len: u64) -> io::Result<&mut File>;
    fn flush(&mut self);
    fn close(&mut self);
}

#[derive(Debug)]
struct SizeRotate {
    path: String,                      // The path where the file will be written
    receiver: mpsc::Receiver<Vec<u8>>, // The receiver for receiving byte vectors
    size_limit: u64,                   // The maximum size limit for the file
    cur_size: u64,                     // The current size of the file
    file: RefCell<Option<File>>,       // The file being written (wrapped in a RefCell)
    compress: bool,                    // Whether to compress the file
    keep_days: i64,                    // The number of days to keep the log files
}

impl SizeRotate {
    fn new(
        path: String,
        receiver: mpsc::Receiver<Vec<u8>>,
        file_size: Option<u64>,
        compress: bool,
        keep_days: i64,
    ) -> Self {
        let slo = file_size.or_else(|| Some(1024 * 1024 * 20)); // If file_size is None, set it to 20MB (default)

        Self {
            path,
            receiver,
            size_limit: slo.unwrap(),
            cur_size: 0,
            file: RefCell::default(),
            compress,
            keep_days,
        }
    }
}

impl Rotate for SizeRotate {
    fn receiver(&mut self) -> &mut mpsc::Receiver<Vec<u8>> {
        &mut self.receiver
    }

    // Get a mutable reference to a file for writing data.
    // If the file is not already open, it opens it and checks the size limit.
    // If the size limit is exceeded, it performs file rotation by flushing the file,
    // renaming it, and recursively calling `get_file` to get a new file.
    fn get_file(&mut self, len: u64) -> io::Result<&mut File> {
        if self.file.get_mut().is_none() {
            let (fp, exists) = open_file(self.path.as_str())?;
            self.file.replace(Some(fp));
            if let Some(meta) = exists {
                self.cur_size = meta.len();
            } else {
                self.cur_size = 0;
            }
        }
        if self.cur_size + len <= self.size_limit {
            // Check if the current size plus the new length is less than or equal to the size limit
            self.cur_size += len;
            return Ok(self.file.get_mut().as_mut().unwrap());
        }

        let mut fp = self.file.take().unwrap();
        if let Err(err) = fp.flush() {
            log!("failed to flush the file: {:+?}", err);
        }

        drop(fp);

        let new_filename = self.rotate_filename(&self.path, self.compress, true);
        log!("move file: {:?} -> {:?}", self.path, new_filename);
        if let Err(err) = fs::rename(self.path.clone(), &new_filename) {
            log!("failed to move the file: {:+?}", err)
        } else {
            if self.compress {
                gzip_encode(&new_filename)?;
            }
        }

        // drop the expired file
        let expire_day = date_add(-self.keep_days);
        remove_log_files(&self.path, &expire_day);

        self.get_file(len)
    }

    // Flushes the file to disk.
    #[inline]
    fn flush(&mut self) {
        let fp = self.file.borrow_mut();
        file_flush(&fp);
    }

    // Closes the file by flushing it and dropping the file handle.
    #[inline]
    fn close(&mut self) {
        self.flush();
        drop(self.file.take());
    }
}

unsafe impl Send for SizeRotate {}

#[derive(Debug)]
struct DailyRotate {
    path: String,                      // The path where the rotated files will be stored
    receiver: mpsc::Receiver<Vec<u8>>, // The receiver end of a channel that receives byte vectors
    file: RefCell<Option<File>>,       // A mutable reference to an optional file
    compress: bool,                    // Whether to compress the rotated files
    keep_days: i64,                    // The number of days to keep rotated files
    create_day: String,                // The day when the file was created
}

impl DailyRotate {
    // Constructs a new instance of DailyRotate
    fn new(
        path: String,
        receiver: mpsc::Receiver<Vec<u8>>,
        compress: bool,
        keep_days: i64,
    ) -> Self {
        Self {
            path,
            receiver,
            file: RefCell::default(),
            compress,
            keep_days,
            create_day: String::new(),
        }
    }
}

impl Rotate for DailyRotate {
    // Receives data from the receiver channel
    // Returns the received data or an error
    fn receiver(&mut self) -> &mut mpsc::Receiver<Vec<u8>> {
        &mut self.receiver
    }

    // Gets the file to write data to
    // If the file is not open, it opens the file and sets the create_day field
    // If the current day is different from the create_day, it rotates the file by flushing, renaming, and opening a new file
    // Returns a mutable reference to the file
    fn get_file(&mut self, len: u64) -> io::Result<&mut File> {
        let day = day(); // Get the current day
        if self.file.get_mut().is_none() {
            let (fp, exists) = open_file(self.path.as_str())?; // Open the file
            self.file.replace(Some(fp)); // Replace the file with the opened file
            if let Some(meta) = exists {
                let date_time: DateTime<Local> = DateTime::from(meta.modified()?);
                self.create_day = date_time.format(DATE_FMT).to_string(); // Set the create_day field based on the file creation time
            } else {
                self.create_day = day.clone(); // Set the create_day field to the current day
            }
        }

        if self.create_day == day {
            return Ok(self.file.get_mut().as_mut().unwrap()); // Return a mutable reference to the file
        }

        let mut fp = self.file.take().unwrap();
        if let Err(err) = fp.flush() {
            log!("failed to flush the file: {:+?}", err);
        }

        drop(fp);

        let new_filename = self.rotate_filename(&self.path, self.compress, false);
        log!("move file: {:?} -> {:?}", self.path, new_filename);
        if let Err(err) = fs::rename(self.path.clone(), &new_filename) {
            log!("failed to move the file: {:+?}", err);
        } else {
            if self.compress {
                gzip_encode(&new_filename)?;
            }
        }
        // drop the expired file
        let expire_day = date_add(-self.keep_days);
        remove_log_files(&self.path, &expire_day);
        self.get_file(len)
    }

    // Flushes the current file
    #[inline]
    fn flush(&mut self) {
        let fp = self.file.borrow_mut();
        file_flush(&fp);
    }

    // Closes the file by flushing it and dropping the file handle
    #[inline]
    fn close(&mut self) {
        self.flush();
        drop(self.file.take());
    }
}

unsafe impl Send for DailyRotate {}

pub fn new(
    file_path: Option<String>,
    mode: CutMode,
    file_size: Option<u64>,
    compress: bool,
    keep_days: i64,
    receiver: mpsc::Receiver<Vec<u8>>,
) -> Box<dyn Rotate + Send> {
    let log_path = match file_path {
        Some(s) => s,
        None => String::from("logs/out"),
    };
    if let Some(log_dir) = path::Path::new(&log_path).parent() {
        if !log_dir.exists() {
            if let Err(err) = fs::create_dir_all(log_dir) {
                panic!("failed to create log directory: {:+?}", err)
            }
        }
    }
    match mode {
        CutMode::Size => {
            let r = SizeRotate::new(log_path, receiver, file_size, compress, keep_days);
            Box::new(r)
        }
        CutMode::Daily => {
            let r = DailyRotate::new(log_path, receiver, compress, keep_days);
            Box::new(r)
        }
    }
}

fn write_all(rotate: &mut Box<dyn Rotate + Send>, data: &[u8]) {
    match rotate.get_file(data.len() as u64) {
        Ok(fp) => {
            if let Err(err) = fp.write_all(data) {
                log!("failed to write content to file: {:+?}", err);
            }
        }
        Err(err) => {
            log!("failed to open file: {:+?}", err);
        }
    }
}

pub async fn start(
    file_path: Option<String>,
    cut_mode: CutMode,
    file_size: Option<u64>,
    compress: bool,
    keep_days: i64,
    receiver: mpsc::Receiver<Vec<u8>>,
    ch: broadcast::Sender<()>,
) {
    let mut rotate = new(
        file_path, cut_mode, file_size, compress, keep_days, receiver,
    );
    let mut tail = None;
    loop {
        match rotate.receiver().recv().await {
            Some(mut data) => {
                let last_tail = tail.take();
                if data[data.len() - 1] != b'\n' {
                    if let Some(index) = data.iter().rposition(|&x| x == b'\n') {
                        tail = Some(data[index + 1..].to_vec());
                        data.truncate(index + 1);
                    }
                }
                let mut lines = utils::Lines::new(data.as_slice());
                if last_tail.is_some() {
                    if let Some(i) = lines.next() {
                        let mut line = last_tail.unwrap();
                        line.append(&mut i.to_vec());
                        write_all(&mut rotate, line.as_slice());
                    } else {
                        if tail.is_some() {
                            let mut t1 = last_tail.unwrap();
                            let mut t2 = tail.unwrap();
                            t1.append(&mut t2);
                            tail = Some(t1);
                        }
                        continue;
                    }
                }
                lines.for_each(|line| {
                    write_all(&mut rotate, line);
                });
            }
            None => {
                break;
            }
        }
    }
    if let Some(t) = tail {
        write_all(&mut rotate, &t);
    }
    rotate.close();
    log!("closed rotation!");
    let _ = ch.send(());
}
