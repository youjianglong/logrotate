#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("logrotate-{name}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_logrotate")
}

fn command() -> Command {
    let mut command = Command::new(binary());
    for variable in [
        "LOG_ROTATE_CONFIG",
        "LOG_ROTATE_OUTPUT",
        "LOG_ROTATE_STDOUT_OUTPUT",
        "LOG_ROTATE_STDERR_OUTPUT",
        "LOG_ROTATE_CUT_MODE",
        "LOG_KEEP_DAYS",
        "LOG_ROTATE_FILE_SIZE",
        "LOG_ROTATE_COMPRESS",
        "LOG_ROTATE_TIMESTAMP_PREFIX",
        "LOG_ROTATE_STREAM_LABEL",
        "LOG_ROTATE_DEBUG",
        "LOG_ROTATE_RESTART",
        "LOG_ROTATE_RESTART_COUNT",
        "LOG_ROTATE_RESTART_INTERVAL",
    ] {
        command.env_remove(variable);
    }
    command
}

fn contents(path: impl AsRef<Path>) -> Vec<u8> {
    fs::read(path).unwrap()
}

#[test]
fn splits_stdout_and_stderr_and_forwards_exit_code() {
    let dir = TestDir::new("split");
    let stdout = dir.join("stdout.log");
    let stderr = dir.join("stderr.log");
    let status = command()
        .args(["--stdout-output"])
        .arg(&stdout)
        .args(["--stderr-output"])
        .arg(&stderr)
        .arg("--stream-label")
        .args([
            "--",
            "sh",
            "-c",
            "printf 'out\\n'; printf 'err\\n' >&2; exit 7",
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(7));
    assert_eq!(contents(stdout), b"out\n");
    assert_eq!(contents(stderr), b"err\n");
}

#[test]
fn adds_short_labels_only_to_a_shared_output() {
    let dir = TestDir::new("labels");
    let shared = dir.join("shared.log");
    let status = command()
        .args(["--output"])
        .arg(&shared)
        .args(["--stream-label", "--", "sh", "-c"])
        .arg("printf 'out\\n'; printf 'err\\n' >&2")
        .status()
        .unwrap();

    assert!(status.success());
    let log = contents(shared);
    assert!(log.windows(b"[O] out\n".len()).any(|v| v == b"[O] out\n"));
    assert!(log.windows(b"[E] err\n".len()).any(|v| v == b"[E] err\n"));
}

#[test]
fn timestamps_empty_and_unterminated_stdin_lines() {
    let dir = TestDir::new("stdin-time");
    let output = dir.join("stdin.log");
    let mut child = command()
        .args(["--output"])
        .arg(&output)
        .arg("--timestamp-prefix")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"\nlast").unwrap();
    assert!(child.wait().unwrap().success());

    let log = String::from_utf8(contents(output)).unwrap();
    let lines = log.split('\n').collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with('['));
    assert!(lines[0].ends_with("] "));
    assert!(lines[1].starts_with('['));
    assert!(lines[1].ends_with("] last"));
}

#[test]
fn rejects_split_options_in_stdin_mode() {
    let dir = TestDir::new("stdin-split");
    let status = command()
        .arg("--stdout-output")
        .arg(dir.join("out.log"))
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn zero_retention_keeps_rotated_files() {
    let dir = TestDir::new("retention");
    let output = dir.join("rotate.log");
    let status = command()
        .args(["--output"])
        .arg(&output)
        .args(["--file-size", "6B", "--keep-days", "0", "--", "sh", "-c"])
        .arg("printf '12345\\n67890\\n'")
        .status()
        .unwrap();
    assert!(status.success());

    let archives = fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("rotate.log.")
        })
        .count();
    assert_eq!(archives, 1);
    assert_eq!(contents(output), b"67890\n");
}

#[test]
fn supports_current_directory_output_and_removes_expired_archives() {
    let dir = TestDir::new("relative-output");
    let expired = dir.join("app.log.20000101-1");
    fs::write(&expired, b"old").unwrap();

    let status = command()
        .current_dir(&dir.0)
        .args([
            "--output",
            "app.log",
            "--file-size",
            "6B",
            "--keep-days",
            "1",
            "--",
            "sh",
            "-c",
            "printf '12345\\n67890\\n'",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(!expired.exists());
    assert_eq!(contents(dir.join("app.log")), b"67890\n");
}

#[test]
fn command_line_overrides_environment_and_toml() {
    let dir = TestDir::new("precedence");
    let config_output = dir.join("config.log");
    let env_output = dir.join("env.log");
    let cli_output = dir.join("cli.log");
    let config_file = dir.join("logrotate.toml");
    fs::write(
        &config_file,
        format!(
            "output = \"{}\"\nfile_size = \"1.5KB\"\n",
            config_output.display()
        ),
    )
    .unwrap();

    let status = command()
        .arg("--config")
        .arg(&config_file)
        .arg("--output")
        .arg(&cli_output)
        .args(["--", "sh", "-c", "printf cli"])
        .env("LOG_ROTATE_OUTPUT", &env_output)
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(contents(cli_output), b"cli");
    assert!(!env_output.exists());
    assert!(!config_output.exists());

    let status = command()
        .arg("--config")
        .arg(&config_file)
        .args(["--", "sh", "-c", "printf env"])
        .env("LOG_ROTATE_OUTPUT", &env_output)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(contents(env_output), b"env");
    assert!(!config_output.exists());
}

#[test]
fn restarts_abnormal_child_until_it_succeeds() {
    let dir = TestDir::new("restart-success");
    let output = dir.join("restart.log");
    let marker = dir.join("started");
    let script = "if [ -f \"$1\" ]; then printf 'second\\n'; exit 0; else : > \"$1\"; printf 'first\\n'; exit 3; fi";
    let status = command()
        .args([
            "--output",
            output.to_str().unwrap(),
            "--restart",
            "--restart-count",
            "2",
            "--restart-interval",
            "1ms",
            "--",
            "sh",
            "-c",
            script,
            "sh",
            marker.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(contents(output), b"first\nsecond\n");
}

#[test]
fn restart_without_a_count_is_unlimited() {
    let dir = TestDir::new("restart-unlimited");
    let output = dir.join("restart.log");
    let marker = dir.join("started");
    let script = "if [ -f \"$1\" ]; then printf 'done\\n'; exit 0; else : > \"$1\"; exit 3; fi";
    let status = command()
        .args([
            "--output",
            output.to_str().unwrap(),
            "--restart",
            "--restart-interval",
            "1ms",
            "--",
            "sh",
            "-c",
            script,
            "sh",
            marker.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(contents(output), b"done\n");
}

#[test]
fn returns_last_abnormal_status_when_retries_are_exhausted() {
    let dir = TestDir::new("restart-exhausted");
    let output = dir.join("restart.log");
    let marker = dir.join("attempts");
    let script = "n=$(cat \"$1\" 2>/dev/null || echo 0); n=$((n + 1)); printf '%s\\n' \"$n\" > \"$1\"; printf 'attempt%s\\n' \"$n\"; exit 4";
    let status = command()
        .args([
            "--output",
            output.to_str().unwrap(),
            "--restart",
            "--restart-count",
            "1",
            "--restart-interval",
            "1ms",
            "--",
            "sh",
            "-c",
            script,
            "sh",
            marker.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert_eq!(status.code(), Some(4));
    assert_eq!(contents(output), b"attempt1\nattempt2\n");
}

#[test]
fn reads_restart_policy_from_toml() {
    let dir = TestDir::new("restart-config");
    let output = dir.join("restart.log");
    let marker = dir.join("started");
    let config = dir.join("restart.toml");
    fs::write(
        &config,
        format!(
            "output = \"{}\"\nrestart = true\nrestart_count = 1\nrestart_interval = \"1ms\"\nexec = [\"sh\", \"-c\", \"if [ -f $0 ]; then printf 'ok\\\\n'; exit 0; else : > $0; exit 2; fi\", \"{}\"]\n",
            output.display(),
            marker.display()
        ),
    )
    .unwrap();

    let status = command().arg("--config").arg(config).status().unwrap();
    assert!(status.success());
    assert_eq!(contents(output), b"ok\n");
}
