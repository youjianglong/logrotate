extern crate libc;

use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::select;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

async fn handle_out(
    mut out: impl AsyncRead + Unpin,
    sender: mpsc::Sender<Vec<u8>>,
    mut cr: broadcast::Receiver<()>,
) {
    loop {
        let mut buf = Vec::new();
        select! {
            res = out.read_buf(&mut buf) => {// Read the output from the child process
                if buf.len()>0 {
                  if let Err(err) = sender.send(buf).await { // Send the output to the receiver
                      log!("write failed: {:+?}", err);
                  }
                }
                match res {
                    Ok(size) => {
                        if size == 0 {
                          log!("stdio closed");
                          break;
                        }
                    },
                    Err(err) => {
                        log!("read failed: {:+?}", err);
                        break;
                    }
                }
            }
            _ = cr.recv() => {
                break;
            }
        }
    }
}

pub async fn spawn(args: Vec<String>, sender: mpsc::Sender<Vec<u8>>, ch: broadcast::Sender<()>) {
    let mut command = Command::new(args[0].clone());
    command.args(&args[1..]);
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("failed to spawn child process");

    if let Some(stdout) = child.stdout {
        let sender = sender.clone();
        tokio::spawn(handle_out(stdout, sender, ch.subscribe()));
        child.stdout = None;
    }

    if let Some(stderr) = child.stderr {
        let sender = sender.clone();
        tokio::spawn(handle_out(stderr, sender, ch.subscribe()));
        child.stderr = None;
    }

    let mut cr = ch.subscribe();

    select! {
        _ = child.wait() => {
            log!("child exited");
            // ch.send(()).expect("failed to send exit signal");
        },
        _ = cr.recv() => {}
    }
}
