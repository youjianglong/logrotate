mod output;

extern crate clap;

use clap::Parser;
use std::io;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(version)]
#[command(author="XiaoYao<you.jianglong@gmail.com>")]
#[command(about = "Log Rolling and Cutting Tool")]
struct Args {
    #[arg(short, long, help = "Specifies the configuration file")]
    config: Option<String>,

    #[arg(short, long, help = "Specifies the output file")]
    output: Option<String>,

    #[arg(long, short = 'm', default_value = "size", help = "Specifies the cut mode")]
    cut_mode: output::CutMode,

    #[arg(short, long, default_value = "0", help = "Specifies the reserved value")]
    reserved: i32,

    #[arg(long, short = 's', help = "Specifies the file size")]
    file_size: Option<u64>,
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
    let args: Args = Args::parse(); // Parse command-line arguments

    let running = Arc::new(AtomicBool::new(true)); // Create a shared atomic boolean variable for termination signal
    signal(running.clone()); // Register a signal handler for termination signal

    // Create an output writer and a join handle for the output thread
    let (mut writer, jh) =
        output::Output::new(args.output, args.cut_mode, args.file_size, running.clone());

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
                let b = &(buf[0..len]); // Get a slice of the buffer containing the actual input
                if let Err(err) = writer.write(b) {
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
