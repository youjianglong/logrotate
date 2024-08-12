mod output;

extern crate clap;

use clap::{Parser, ValueEnum};
use std::fs::File;
use std::io;
use std::io::{ErrorKind, Read, Write};
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use toml;
use toml::Table;

#[derive(Parser, Debug, Clone)]
#[command(version)]
#[command(author = "XiaoYao<you.jianglong@gmail.com>")]
#[command(about = "Log Rolling and Cutting Tool")]
struct Args {
    #[arg(
        short,
        long,
        env = "LOG_ROTATE_CONFIG",
        help = "Specifies the configuration file"
    )]
    config: Option<String>,

    #[arg(
        short,
        long,
        env = "LOG_ROTATE_OUTPUT",
        help = "Specifies the output file"
    )]
    output: Option<String>,

    #[arg(
        long,
        short = 'm',
        env = "LOG_ROTATE_CUT_MODE",
        default_value = "size",
        help = "Specifies the cut mode"
    )]
    cut_mode: output::CutMode,

    #[arg(
        long,
        short,
        env = "LOG_ROTATE_RESERVED",
        default_value = "0",
        help = "Specifies the reserved value"
    )]
    reserved: i32,

    #[arg(
        long,
        short = 's',
        env = "LOG_ROTATE_FILE_SIZE",
        default_value = "16777216",
        help = "Specifies the file size"
    )]
    file_size: Option<u64>,

    #[arg(
        long,
        short = 'z',
        env = "LOG_ROTATE_COMPRESS",
        default_value = "false",
        help = "Specifies the compression level"
    )]
    compress: bool,

    #[arg(
        long,
        short = 'i',
        env = "LOG_ROTATE_FLUSH",
        default_value = "5",
        help = "Specifies the flush interval"
    )]
    flush_interval: u64,
}

// This function sets up a signal handler for the interrupt signal (Ctrl+C)
// The `running` parameter is an `Arc<AtomicBool>` which allows thread-safe access to the `running` variable
fn signal(running: Arc<AtomicBool>) {
    // Set the interrupt signal handler using the `ctrlc::set_handler` function
    // The closure passed to the `set_handler` function will be called when the interrupt signal is received
    // The closure sets the value of the `running` variable to `false` using the `store` method of the `AtomicBool` struct
    // The `Ordering::SeqCst` argument specifies the memory ordering used by the `store` method
    ctrlc::set_handler(move || running.store(false, Ordering::SeqCst)).unwrap();
}

fn main() {
    let mut args: Args = Args::parse(); // Parse command-line arguments

    let running = Arc::new(AtomicBool::new(true)); // Create a shared atomic boolean variable for termination signal
    signal(running.clone()); // Register a signal handler for termination signal

    if let Some(ref config_file) = args.config {
        // If a configuration file is specified, read and parse it
        let mut file = File::open(config_file.as_str()).expect("Open config file failed");
        let mut buf = String::new();
        let size = file
            .read_to_string(&mut buf)
            .expect("Read config file failed");
        if size == 0 {
            println!("Config file is empty");
            exit(1)
        }
        let table: Table = toml::from_str(buf.as_str()).expect("Parse config file failed");
        if let Some(val) = table.get("output") {
            args.output = Some(val.as_str().expect("\"output\" must be string").to_string());
        }
        if let Some(val) = table.get("cut_mode") {
            args.cut_mode =
                output::CutMode::from_str(val.as_str().expect("\"cut_mode\" must be string"), true)
                    .expect("cut_mode must be valid");
        }
        if let Some(val) = table.get("reserved") {
            args.reserved = val.as_integer().expect("\"reserved\" must be integer") as i32;
        }
        if let Some(val) = table.get("file_size") {
            args.file_size = Some(val.as_integer().expect("\"file_size\" must be integer") as u64);
        }
        if let Some(val) = table.get("compress") {
            args.compress = val.as_bool().expect("\"compress\" must be bool");
        }
        if let Some(val) = table.get("flush_interval") {
            args.flush_interval =
                val.as_integer()
                    .expect("\"flush_interval\" must be integer") as u64;
        }
    }

    // Create an output writer and a join handle for the output thread
    let (mut writer, jh) = output::Output::new(
        args.output,
        args.cut_mode,
        args.file_size,
        args.compress,
        running.clone(),
        args.flush_interval,
    );

    let mut stdin = io::stdin(); // Create a handle for standard input
    let mut buf = [0u8; 1024]; // Create a buffer to read input

    while running.load(Ordering::SeqCst) {
        // Loop until termination signal is received
        let res = stdin.read(&mut buf); // Read input from standard input into the buffer
        match res {
            Ok(len) => {
                // If input was successfully read
                if len < 1 {
                    // If the length of the input is less than 1, continue to the next iteration
                    continue;
                }
                if let Err(err) = writer.write(&(buf[0..len])) {
                    // Write the input to the output file
                    println!("Write failed: {:+?}", err); // Print an error message if the write operation fails
                    continue; // Continue to the next iteration
                }
            }
            Err(err) => match err.kind() {
                // If an error occurred while reading input
                ErrorKind::UnexpectedEof => {
                    // If the error is an unexpected end-of-file
                    println!("Stdin closed");
                    break;
                }
                _ => {
                    println!("Read failed: {:+?}", err)
                }
            },
        }
    }

    println!("Shutting down ...");
    jh.join().unwrap(); // Wait for the output thread to finish and unwrap any errors
}
