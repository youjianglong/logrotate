extern crate libc;

use std::process::Stdio;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::select;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

async fn handle_out(mut out: impl AsyncRead + Unpin, name: &str, sender: mpsc::Sender<Vec<u8>>) {
    loop {
        let mut buf = Vec::new();
        select! {
            res = out.read_buf(&mut buf) => {// Read the output from the child process
                if buf.len()>0 {
                  if let Err(err) = sender.send(buf).await { // Send the output to the receiver
                      log!("{} write failed: {:+?}", name, err);
                  }
                }
                match res {
                    Ok(size) => {
                        if size == 0 {
                          log!("{} closed", name);
                          break;
                        }
                    },
                    Err(err) => {
                        log!("{} read failed: {:+?}", name, err);
                        break;
                    }
                }
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
    let mut joins = Vec::new();

    if let Some(stdout) = child.stdout {
        let sender = sender.clone();
        let join = tokio::spawn(handle_out(stdout, "stdout", sender));
        joins.push(join);
        child.stdout = None;
    }

    if let Some(stderr) = child.stderr {
        let sender = sender.clone();
        let join = tokio::spawn(handle_out(stderr, "stderr", sender));
        joins.push(join);
        child.stderr = None;
    }

    let mut cr = ch.subscribe();

    select! {
        _ = child.wait() => {
            log!("child process exited");
        },
        _ = cr.recv() => {}
    }
    for j in joins {
        match j.await {
            Err(err) => {
                log!("join failed: {:+?}", err);
            }
            _ => {}
        }
    }
}
