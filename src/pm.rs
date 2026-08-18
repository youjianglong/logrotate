use crate::line::{LineEncoder, PrefixOptions, Stream};
use std::io;
use std::process::{ExitStatus, Stdio};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::signal::ctrl_c;
use tokio::sync::mpsc;

async fn send(sender: &mpsc::Sender<Vec<u8>>, records: Vec<Vec<u8>>) -> io::Result<()> {
    for record in records {
        sender.send(record).await.map_err(|_| {
            io::Error::new(io::ErrorKind::BrokenPipe, "log writer stopped unexpectedly")
        })?;
    }
    Ok(())
}

async fn handle_output(
    mut output: impl AsyncRead + Unpin,
    stream: Stream,
    options: PrefixOptions,
    sender: mpsc::Sender<Vec<u8>>,
) -> io::Result<()> {
    let mut encoder = LineEncoder::new(stream, options);
    let mut buffer = [0_u8; 8192];
    loop {
        let size = output.read(&mut buffer).await?;
        if size == 0 {
            break;
        }
        send(&sender, encoder.push(&buffer[..size])).await?;
    }
    if let Some(record) = encoder.finish() {
        send(&sender, vec![record]).await?;
    }
    Ok(())
}

#[cfg(unix)]
fn interrupt_child(child: &mut tokio::process::Child) -> io::Result<()> {
    let pid = child
        .id()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "child process has no pid"))?;
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn interrupt_child(child: &mut tokio::process::Child) -> io::Result<()> {
    child.start_kill()
}

pub async fn spawn(
    command: Vec<String>,
    stdout_sender: mpsc::Sender<Vec<u8>>,
    stderr_sender: mpsc::Sender<Vec<u8>>,
    stdout_options: PrefixOptions,
    stderr_options: PrefixOptions,
) -> io::Result<ExitStatus> {
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = process.spawn().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("failed to start command `{}`: {e}", command[0]),
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture child stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture child stderr"))?;

    let stdout_task = tokio::spawn(handle_output(
        stdout,
        Stream::Stdout,
        stdout_options,
        stdout_sender,
    ));
    let stderr_task = tokio::spawn(handle_output(
        stderr,
        Stream::Stderr,
        stderr_options,
        stderr_sender,
    ));

    let status = tokio::select! {
        status = child.wait() => status?,
        signal = ctrl_c() => {
            signal?;
            if let Err(error) = interrupt_child(&mut child) {
                log!("failed to forward Ctrl+C to child: {error}");
            }
            child.wait().await?
        }
    };

    stdout_task
        .await
        .map_err(|e| io::Error::other(format!("stdout reader task failed: {e}")))??;
    stderr_task
        .await
        .map_err(|e| io::Error::other(format!("stderr reader task failed: {e}")))??;
    Ok(status)
}
