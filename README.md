# logrotate

`logrotate` captures stdin or a child process's output and rotates the resulting
log files by size or by day. Rotated files can be compressed and expired
automatically.

## Usage

Capture stdin into the default `logs/out` file:

```sh
producer | logrotate
```

Capture a command, keeping stdout and stderr together:

```sh
logrotate --output logs/app.log --stream-label -- my-app --serve
```

Write stdout and stderr to independently rotated files:

```sh
logrotate \
  --stdout-output logs/app.log \
  --stderr-output logs/app-error.log \
  --timestamp-prefix \
  -- my-app --serve
```

When only one stream-specific path is supplied, the other stream falls back to
`--output`, and then to `logs/out`. If both paths resolve to the same file they
share one rotation state. `--stream-label` adds `[O]` or `[E]` only in that
shared-file case; it is ignored for split files.

`--timestamp-prefix` adds local time to every line:

```text
[26/08/18 22:45:00.000] [O] application output
```

The `[O]`/`[E]` portion is present only when `--stream-label` is also enabled.

## Sizes and retention

`--file-size` accepts bytes or human-readable sizes. `K`, `M`, `G`, and `T`
units always use powers of 1024, including their `KB` and `KiB` spellings:

```sh
logrotate --file-size 16MB --output logs/app.log -- my-app
logrotate --file-size 1.5GiB --output logs/app.log -- my-app
```

Fractional bytes are rounded down. A zero result, negative value, unknown unit,
or overflow is rejected. `--keep-days 0` (the default) disables automatic
cleanup; positive values enable expiry. `--compress` gzip-compresses archives.

## Configuration

Options can be provided in TOML:

```toml
output = "logs/app.log"
stdout_output = "logs/app.log"
stderr_output = "logs/app-error.log"
cut_mode = "size"
file_size = "16MB"
keep_days = 14
compress = true
timestamp_prefix = true
stream_label = false
exec = ["my-app", "--serve"]
```

Load it with `logrotate --config logrotate.toml`. Precedence is command-line
arguments, environment variables, TOML, then built-in defaults. The legacy TOML
key `keep_num` remains accepted when `keep_days` is absent.

Run `logrotate --help` for the environment-variable names and all options.

## Child-process restart

Enable supervised restarts for abnormal child exits with `--restart`. The
restart count is the number of retries after the initial process. If
`--restart-count` is omitted, retries are unlimited; an explicitly supplied
`0` disables retries. The interval accepts `ms`, `s`, `m`, or `h` and defaults to
`1s`:

```sh
logrotate --restart --restart-count 5 --restart-interval 2s -- my-app
```

The same settings can be placed in TOML or environment variables:

```toml
restart = true
# Omit restart_count for unlimited retries.
restart_count = 5
restart_interval = "2s"
```

They correspond to `LOG_ROTATE_RESTART`, `LOG_ROTATE_RESTART_COUNT`, and
`LOG_ROTATE_RESTART_INTERVAL`. A normal exit (code 0) is never restarted. When
the retry limit is reached, the last abnormal exit code is returned. Ctrl+C
cancels the supervision loop and does not start another child.

## Exit status

In command mode, `logrotate` returns the child process's exit code. On Unix, a
signal exit maps to `128 + signal`. Invalid configuration, process startup, or
log I/O errors return a non-zero status. Ctrl+C is forwarded to the child before
the captured output is drained and flushed.
