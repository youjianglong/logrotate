use chrono::{DateTime, Local};
use clap::ValueEnum;
use fs::File;
use libflate::gzip::Encoder;
use std::cell::RefCell;
use std::fmt::Debug;
use std::fs;
use std::io;
use std::io::{copy, Error, ErrorKind, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time;
use std::vec::Vec;

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum CutMode {
    Size,  // Represents the mode for cutting logs based on size
    Daily, // Represents the mode for cutting logs on a daily basis
}

const DATE_FMT: &str = "%Y-%m-%d"; // Date format: Year-Month-Day

// Define a trait named "Rotate"
trait Rotate {
    // Returns the current day as a string in a specific format
    fn day(&self) -> String {
        Local::now().format(DATE_FMT).to_string()
    }

    // Checks if a file exists at the given path
    fn is_file(&self, path: &String) -> bool {
        return fs::metadata(path).is_ok_and(|meta| meta.is_file());
    }

    // Rotates the filename by appending the current day to it
    // If the rotated filename already exists, it appends a unique identifier to it
    fn rotate_filename(&self, path: &String, compress: bool, mul: bool) -> String {
        let day = self.day();
        if !mul {
            let filename = path.clone() + "." + day.as_str();
            if compress && !self.gzip_exists(&filename) || !compress && !self.is_file(&filename) {
                return filename;
            }
        }
        let mut i = 1;
        loop {
            let filename = format!("{:}.{:}.{:}", path, day, i);
            if (compress && !self.gzip_exists(&filename)) || (!compress && !self.is_file(&filename))
            {
                return filename;
            }
            i += 1;
        }
    }

    // Opens a file at the given path and returns a tuple containing the file handle and its metadata
    // If the file does not exist, it creates a new file and returns the file handle without metadata
    // Prints an error message if there is an error opening or creating the file
    fn open_file(&self, path: &str) -> io::Result<(File, Option<fs::Metadata>)> {
        match fs::metadata(path) {
            Ok(meta) => File::options()
                .append(true)
                .open(path)
                .map(move |fp| (fp, Some(meta))),
            Err(err) => match err.kind() {
                ErrorKind::NotFound => File::create(path).map(|fp| (fp, None)),
                _ => {
                    println!("Failed to read file metadata: {:+?}", err);
                    Err(err)
                }
            },
        }
    }

    // Flushes the contents of the file handle to disk
    // Prints an error message if there is an error flushing the file
    fn file_flush(&self, file: &Option<File>) {
        if let Some(mut fp) = file.as_ref() {
            if let Err(err) = fp.flush() {
                println!("Failed to flush file: {:+?}", err)
            }
        }
    }

    // Synchronizes the data by continuously receiving it and writing it to the file
    // Prints an error message if there is an error writing the data to the file
    fn sync(&mut self) {
        loop {
            match self.receive() {
                Ok(data) => match self.get_file(&data) {
                    Ok(fp) => {
                        if let Err(err) = fp.write_all(data.as_slice()) {
                            println!("Failed to write content to file: {:+?}", err);
                            break;
                        }
                    }
                    Err(err) => {
                        println!("Failed to open file: {:+?}", err);
                    }
                },
                Err(err) => match err {
                    TryRecvError::Empty => break,
                    TryRecvError::Disconnected => {
                        println!("Pipe disconnected: {:+?}", err);
                        break;
                    }
                },
            }
        }
    }

    // Runs the synchronization process at a timed interval
    // Takes two atomic bools as parameters: `running` for controlling the loop and `flushing` for indicating if a flush is needed
    // Sleeps for one second, then calls the `sync()` method to synchronize the data
    // If the `flushing` bool is true, it calls the `flush()` method to flush the file contents
    // Prints a message when starting the flush process
    fn timed(&mut self, running: Arc<AtomicBool>, flushing: Arc<AtomicBool>, interval: u64) {
        let dur = time::Duration::from_secs(interval);
        while running.load(Ordering::SeqCst) {
            thread::sleep(dur);
            self.sync();
            if flushing.swap(false, Ordering::SeqCst) {
                self.flush();
            }
        }
        self.sync();
        self.close();
    }

    fn gzip_encode(&self, filename: &String) -> io::Result<()> {
        let mut inp = File::open(filename)?;
        let out = File::create(format!("{}.gz", filename))?;
        let mut encoder = Encoder::new(out)?;
        copy(&mut inp, &mut encoder)?;
        drop(inp);
        drop(encoder.finish().into_result()?);
        fs::remove_file(filename)?;
        Ok(())
    }

    fn gzip_exists(&self, filename: &String) -> bool {
        self.is_file(&format!("{}.gz", filename))
    }

    fn receive(&mut self) -> Result<Vec<u8>, TryRecvError>;
    fn get_file(&mut self, data: &Vec<u8>) -> io::Result<&mut File>;
    fn flush(&mut self);
    fn close(&mut self);
}

#[derive(Debug)]
struct SizeRotate {
    path: String,                // The path where the file will be written
    receiver: Receiver<Vec<u8>>, // The receiver for receiving byte vectors
    size_limit: u64,             // The maximum size limit for the file
    cur_size: u64,               // The current size of the file
    file: RefCell<Option<File>>, // The file being written (wrapped in a RefCell)
    compress: bool,              // Whether to compress the file
}

impl SizeRotate {
    fn new(
        path: String,
        receiver: Receiver<Vec<u8>>,
        file_size: Option<u64>,
        compress: bool,
    ) -> Self {
        let slo = file_size.or_else(|| Some(1024 * 1024 * 20)); // If file_size is None, set it to 20MB (default)

        Self {
            path,                     // Initialize path field with provided path argument
            receiver,                 // Initialize receiver field with provided receiver argument
            size_limit: slo.unwrap(), // Initialize size_limit field with the value of slo
            cur_size: 0,              // Initialize cur_size field with 0
            file: RefCell::default(), // Initialize file field with a default value
            compress, // Initialize compress field with the provided compress argument
        }
    }
}

impl Rotate for SizeRotate {
    fn receive(&mut self) -> Result<Vec<u8>, TryRecvError> {
        return self.receiver.try_recv();
    }

    // Get a mutable reference to a file for writing data.
    // If the file is not already open, it opens it and checks the size limit.
    // If the size limit is exceeded, it performs file rotation by flushing the file,
    // renaming it, and recursively calling `get_file` to get a new file.
    fn get_file(&mut self, data: &Vec<u8>) -> io::Result<&mut File> {
        if self.file.get_mut().is_none() {
            let (fp, exists) = self.open_file(self.path.as_str())?;
            self.file.replace(Some(fp));
            if let Some(meta) = exists {
                self.cur_size = meta.len();
            } else {
                self.cur_size = 0;
            }
        }
        let len = data.len() as u64;
        if self.cur_size.clone() + len <= self.size_limit || len >= self.size_limit {
            self.cur_size += len.clone();
            return Ok(self.file.get_mut().as_mut().unwrap());
        }

        let mut fp = self.file.take().unwrap();
        if let Err(err) = fp.flush() {
            println!("Failed to flush the file: {:+?}", err);
        }
        drop(fp);
        let new_filename = self.rotate_filename(&self.path, self.compress, true);
        println!("Move file: {:?} -> {:?}", self.path, new_filename);
        if let Err(err) = fs::rename(self.path.clone(), &new_filename) {
            println!("Failed to move the file: {:+?}", err)
        } else {
            if self.compress {
                self.gzip_encode(&new_filename)?;
            }
        }
        self.get_file(data)
    }

    // Flushes the file to disk.
    #[inline]
    fn flush(&mut self) {
        let fp = self.file.borrow_mut();
        self.file_flush(&fp);
    }

    // Closes the file by flushing it and dropping the file handle.
    #[inline]
    fn close(&mut self) {
        self.flush();
        drop(self.file.take());
    }
}

#[derive(Debug)]
struct DailyRotate {
    path: String,                // The path where the rotated files will be stored
    receiver: Receiver<Vec<u8>>, // The receiver end of a channel that receives byte vectors
    file: RefCell<Option<File>>, // A mutable reference to an optional file
    create_day: String,          // The day when the file was created
    compress: bool,              // Whether to compress the rotated files
}

impl DailyRotate {
    // Constructs a new instance of DailyRotate
    fn new(path: String, receiver: Receiver<Vec<u8>>, compress: bool) -> Self {
        Self {
            path,                      // Assigns the given path to the path field
            receiver,                  // Assigns the given receiver to the receiver field
            file: RefCell::default(),  // Initializes the file field as an empty option
            create_day: String::new(), // Initializes the create_day field as an empty string
            compress,                  // Assigns the given compress flag to the compress field
        }
    }
}

impl Rotate for DailyRotate {
    // Receives data from the receiver channel
    // Returns the received data or an error
    fn receive(&mut self) -> Result<Vec<u8>, TryRecvError> {
        return self.receiver.try_recv();
    }

    // Gets the file to write data to
    // If the file is not open, it opens the file and sets the create_day field
    // If the current day is different from the create_day, it rotates the file by flushing, renaming, and opening a new file
    // Returns a mutable reference to the file
    fn get_file(&mut self, data: &Vec<u8>) -> io::Result<&mut File> {
        let day = self.day(); // Get the current day
        if self.file.get_mut().is_none() {
            let (fp, exists) = self.open_file(self.path.as_str())?; // Open the file
            self.file.replace(Some(fp)); // Replace the file with the opened file
            if let Some(meta) = exists {
                let datetime: DateTime<Local> = DateTime::from(meta.created()?);
                self.create_day = datetime.format(DATE_FMT).to_string(); // Set the create_day field based on the file creation time
            } else {
                self.create_day = day.clone(); // Set the create_day field to the current day
            }
        }

        if self.create_day == day {
            return Ok(self.file.get_mut().as_mut().unwrap()); // Return a mutable reference to the file
        }

        let mut fp = self.file.take().unwrap();
        if let Err(err) = fp.flush() {
            println!("Failed to flush the file: {:+?}", err);
        }
        drop(fp);

        let new_filename = self.rotate_filename(&self.path, self.compress, false);
        println!("Move file: {:} -> {:}", self.path, new_filename);
        if let Err(err) = fs::rename(self.path.clone(), &new_filename) {
            println!("Failed to move the file: {:+?}", err);
        } else {
            if self.compress {
                self.gzip_encode(&new_filename)?;
            }
        }
        self.get_file(data)
    }

    // Flushes the current file
    #[inline]
    fn flush(&mut self) {
        let fp = self.file.borrow_mut();
        self.file_flush(&fp);
    }

    // Closes the file by flushing it and dropping the file handle
    #[inline]
    fn close(&mut self) {
        self.flush();
        drop(self.file.take());
    }
}

#[derive(Debug)]
pub(crate) struct Output {
    sender: SyncSender<Vec<u8>>, // Sender for sending log data
    flushing: Arc<AtomicBool>,   // Atomic boolean flag for indicating if a flush is requested
}

impl Output {
    /// Creates a new `Output` instance.
    ///
    /// # Arguments
    ///
    /// * `path` - An optional path for the log files.
    /// * `mode` - The `CutMode` for cutting log files.
    /// * `file_size` - An optional maximum size for log files.
    /// * `running` - An `Arc<AtomicBool>` indicating if the logger is running.
    ///
    /// # Returns
    ///
    /// Returns a tuple containing the `Output` instance and a `JoinHandle<()>`.
    pub fn new(
        path: Option<String>,
        mode: CutMode,
        file_size: Option<u64>,
        compress: bool,
        running: Arc<AtomicBool>,
        interval: u64,
    ) -> (Self, JoinHandle<()>) {
        let log_path = match path {
            Some(s) => s,
            None => String::from("output"),
        };
        let (sender, receiver) = sync_channel(64);
        let flushing = Arc::new(AtomicBool::new(false));
        let fsh = flushing.clone();
        let jh = thread::spawn(move || match mode {
            CutMode::Size => SizeRotate::new(log_path, receiver, file_size, compress)
                .timed(running, fsh, interval),
            CutMode::Daily => {
                DailyRotate::new(log_path, receiver, compress).timed(running, fsh, interval)
            }
        });

        let output = Self { sender, flushing };

        (output, jh)
    }
}

impl Write for Output {
    /// Writes the provided buffer to the log sender.
    ///
    /// # Arguments
    ///
    /// * `buf` - A slice representing the buffer to be written.
    ///
    /// # Returns
    ///
    /// Returns an `io::Result` indicating the success of the write operation.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Err(err) = self.sender.send(buf.to_vec()) {
            return Err(Error::new(ErrorKind::Other, err));
        }
        Ok(buf.len())
    }

    /// Flushes the log sender by setting the flushing flag to true.
    ///
    /// # Returns
    ///
    /// Returns an `io::Result` indicating the success of the flush operation.
    fn flush(&mut self) -> io::Result<()> {
        self.flushing.store(true, Ordering::SeqCst);
        Ok(())
    }
}
