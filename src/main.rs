extern crate clap;

#[macro_use]
mod utils;
mod pm;
mod rotate;

use clap::{Parser, ValueEnum};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::process::exit;
use tokio::io::{stdin, AsyncReadExt};
use tokio::signal::ctrl_c;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio::{join, select};
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
    cut_mode: rotate::CutMode,

    #[arg(
        long,
        short,
        env = "LOG_KEEP_DAYS",
        default_value = "0",
        help = "Specifies the log retention period"
    )]
    keep_days: i64,

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
        env = "LOG_ROTATE_DEBUG",
        default_value = "false",
        help = "Specifies the debug switch"
    )]
    debug: bool,

    #[clap(
        value_name = "ARGS",
        help = "Arguments to pass to the target process, first is the command, others are arguments, like: `ls -l -a`"
    )]
    args: Vec<String>,
}

// This function sets up a signal handler for the interrupt signal (Ctrl+C)
// The `running` parameter is an `Arc<AtomicBool>` which allows thread-safe access to the `running` variable
async fn signal(ch: broadcast::Sender<()>) {
    let mut cr = ch.subscribe();
    select! {
      s = ctrl_c() => {
        // The `ctrl_c()` function returns a future that resolves when the interrupt signal is received
        match s {
            Ok(()) => {
                log!("interrupted");
                sleep(Duration::from_millis(500)).await;
                ch.send(()).expect("broadcast send error");
            }
            Err(err) => {
                log!("signal error: {}", err);
            }
        }
      },
      _ = cr.recv() => {}
    }
}

fn parse_args() -> Args {
    let mut args: Args = Args::parse(); // Parse command-line arguments
    if let Some(ref config_file) = args.config {
        // If a configuration file is specified, read and parse it
        let mut file = File::open(config_file.as_str()).expect("Open config file failed");
        let mut buf = String::new();
        let size = file
            .read_to_string(&mut buf)
            .expect("Read config file failed");
        if size == 0 {
            log!("Config file is empty");
            exit(1)
        }
        let table: Table = toml::from_str(buf.as_str()).expect("Parse config file failed");
        if let Some(val) = table.get("output") {
            args.output = Some(val.as_str().expect("\"output\" must be string").to_string());
        }
        if let Some(val) = table.get("cut_mode") {
            args.cut_mode =
                rotate::CutMode::from_str(val.as_str().expect("\"cut_mode\" must be string"), true)
                    .expect("cut_mode must be valid");
        }
        if let Some(val) = table.get("keep_num") {
            args.keep_days = val.as_integer().expect("\"keep_num\" must be integer");
        }
        if let Some(val) = table.get("file_size") {
            args.file_size = Some(val.as_integer().expect("\"file_size\" must be integer") as u64);
        }
        if let Some(val) = table.get("compress") {
            args.compress = val.as_bool().expect("\"compress\" must be bool");
        }
        if let Some(val) = table.get("debug") {
            args.debug = val.as_bool().expect("\"debug\" must be bool");
        }
        if let Some(val) = table.get("exec") {
            args.args = val
                .as_array()
                .expect("\"exec\" must be array of string")
                .iter()
                .map(|x| {
                    x.as_str()
                        .expect("\"exec\" must be array of string")
                        .to_string()
                })
                .collect();
        }
    }
    args
}

async fn stdin_read(sender: mpsc::Sender<Vec<u8>>, ch: broadcast::Sender<()>) {
    let mut stdin = stdin(); // Create a handle to the standard input
    let mut cr = ch.subscribe();
    loop {
        let mut buf = Vec::new(); // Create a buffer to read input
        select! {
          res = stdin.read_buf(&mut buf) => {
            if buf.len() > 0 {
                if let Err(err) = sender.send(buf).await {
                    // Write the input to the output file
                    log!("write failed: {:+?}", err); // Print an error message if the write operation fails
                }
            }
            match res {
                Ok(len) => { // If input was successfully read
                    if len < 1 { // If the length of the input is less than 1, it means that the input has been closed
                        log!("stdin closed");
                        break;
                    }
                }
                Err(err) => match err.kind() {
                    // If an error occurred while reading input
                    ErrorKind::UnexpectedEof => {
                        // If the error is an unexpected end-of-file
                        log!("stdin closed");
                        break;
                    }
                    _ => {
                        log!("read failed: {:+?}", err);
                        break;
                    }
                },
            }
          },
          _ = cr.recv() => {
            break;
          }
        }
    }
    log!("finish stdin read!");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = parse_args();

    utils::set_debug(args.debug);

    let (sender, receiver) = mpsc::channel::<Vec<u8>>(64);
    let (ch, _) = broadcast::channel(3);

    let src_handle = async {
        if args.args.len() > 0 {
            pm::spawn(args.args, sender, ch.clone()).await;
        } else {
            stdin_read(sender, ch.clone()).await;
        };
    };

    join!(
        rotate::start(
            args.output,
            args.cut_mode,
            args.file_size,
            args.compress,
            args.keep_days,
            receiver,
            ch.clone()
        ),
        src_handle,
        signal(ch.clone())
    );
    drop(ch);
    exit(0);
}
