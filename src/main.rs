#[macro_use]
mod utils;

mod config;
mod line;
mod pm;
mod rotate;

use crate::config::RuntimeConfig;
use crate::line::{LineEncoder, PrefixOptions, Stream};
use std::error::Error;
use std::io;
use std::path::Path;
use std::process::ExitStatus;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::sleep;

type AppResult<T> = Result<T, Box<dyn Error>>;
type Writer = (mpsc::Sender<Vec<u8>>, JoinHandle<io::Result<()>>);

fn output_path(path: &Path) -> io::Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("output path is not valid UTF-8: {}", path.display()),
        )
    })
}

fn start_writer(path: &Path, config: &RuntimeConfig) -> io::Result<Writer> {
    let (sender, receiver) = mpsc::channel(64);
    let path = output_path(path)?;
    let cut_mode = config.cut_mode.clone();
    let file_size = config.file_size;
    let compress = config.compress;
    let keep_days = config.keep_days;
    let task = tokio::spawn(async move {
        rotate::start(
            Some(path),
            cut_mode,
            Some(file_size),
            compress,
            keep_days,
            receiver,
        )
        .await
    });
    Ok((sender, task))
}

async fn wait_writer(task: JoinHandle<io::Result<()>>) -> io::Result<()> {
    task.await
        .map_err(|e| io::Error::other(format!("log writer task failed: {e}")))?
}

async fn send_records(sender: &mpsc::Sender<Vec<u8>>, records: Vec<Vec<u8>>) -> io::Result<()> {
    for record in records {
        sender.send(record).await.map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "log writer stopped unexpectedly")
        })?;
    }
    Ok(())
}

async fn run_stdin(config: &RuntimeConfig) -> AppResult<i32> {
    let (sender, writer) = start_writer(&config.output, config)?;
    let mut stdin = tokio::io::stdin();
    let mut encoder = LineEncoder::new(
        Stream::Stdin,
        PrefixOptions {
            timestamp: config.timestamp_prefix,
            stream_label: false,
        },
    );
    let mut buffer = [0_u8; 8192];
    let mut exit_code = 0;

    loop {
        tokio::select! {
            read = stdin.read(&mut buffer) => {
                let size = read?;
                if size == 0 {
                    break;
                }
                send_records(&sender, encoder.push(&buffer[..size])).await?;
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                exit_code = 130;
                break;
            }
        }
    }
    if let Some(record) = encoder.finish() {
        send_records(&sender, vec![record]).await?;
    }
    drop(sender);
    wait_writer(writer).await?;
    Ok(exit_code)
}

async fn supervise(
    config: &RuntimeConfig,
    stdout_sender: &mpsc::Sender<Vec<u8>>,
    stderr_sender: &mpsc::Sender<Vec<u8>>,
    stdout_options: PrefixOptions,
    stderr_options: PrefixOptions,
) -> AppResult<i32> {
    let mut retries = 0;
    loop {
        let result = pm::spawn(
            config.command.clone(),
            stdout_sender.clone(),
            stderr_sender.clone(),
            stdout_options,
            stderr_options,
        )
        .await;

        match result {
            Ok(outcome) => {
                if outcome.interrupted || outcome.status.success() || !config.restart {
                    return Ok(exit_code(outcome.status));
                }
                if config
                    .restart_count
                    .is_some_and(|restart_count| retries >= restart_count)
                {
                    return Ok(exit_code(outcome.status));
                }
                retries += 1;
                let retry_limit = config
                    .restart_count
                    .map_or_else(|| "unlimited".to_string(), |count| count.to_string());
                log!(
                    "child exited abnormally; restarting ({}/{})",
                    retries,
                    retry_limit
                );
            }
            Err(error) => {
                if !config.restart
                    || config
                        .restart_count
                        .is_some_and(|restart_count| retries >= restart_count)
                {
                    return Err(error.into());
                }
                retries += 1;
                let retry_limit = config
                    .restart_count
                    .map_or_else(|| "unlimited".to_string(), |count| count.to_string());
                log!(
                    "child failed to run: {error}; restarting ({}/{})",
                    retries,
                    retry_limit
                );
            }
        }

        tokio::select! {
            _ = sleep(config.restart_interval) => {},
            signal = tokio::signal::ctrl_c() => {
                signal?;
                return Ok(130);
            }
        }
    }
}

async fn run_process(config: &RuntimeConfig) -> AppResult<i32> {
    let stdout_path = config.stdout_path();
    let stderr_path = config.stderr_path();
    let shared = stdout_path == stderr_path;
    let prefix = |stream_label| PrefixOptions {
        timestamp: config.timestamp_prefix,
        stream_label,
    };

    let (process_result, writer_result) = if shared {
        let (sender, writer) = start_writer(&stdout_path, config)?;
        let process_result = supervise(
            config,
            &sender,
            &sender,
            prefix(config.stream_label),
            prefix(config.stream_label),
        )
        .await;
        drop(sender);
        let writer_result = wait_writer(writer).await;
        (process_result, writer_result)
    } else {
        let (stdout_sender, stdout_writer) = start_writer(&stdout_path, config)?;
        let (stderr_sender, stderr_writer) = start_writer(&stderr_path, config)?;
        let process_result = supervise(
            config,
            &stdout_sender,
            &stderr_sender,
            prefix(false),
            prefix(false),
        )
        .await;
        drop(stdout_sender);
        drop(stderr_sender);
        let (stdout_result, stderr_result) =
            tokio::join!(wait_writer(stdout_writer), wait_writer(stderr_writer));
        (process_result, stdout_result.and(stderr_result))
    };

    writer_result?;
    process_result
}

fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

async fn run() -> AppResult<i32> {
    let config = config::load()?;
    utils::set_debug(config.debug);
    log!("configuration loaded");
    if config.is_process_mode() {
        run_process(&config).await
    } else {
        run_stdin(&config).await
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    match run().await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("logrotate: {error}");
            std::process::exit(1);
        }
    }
}
