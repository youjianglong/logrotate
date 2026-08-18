use crate::rotate::CutMode;
use clap::{Parser, ValueEnum};
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use toml::Table;

const DEFAULT_OUTPUT: &str = "logs/out";
const DEFAULT_FILE_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteSize(pub u64);

impl FromStr for ByteSize {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            return Err("size cannot be empty".into());
        }

        let split = value
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(value.len());
        let (number, suffix) = value.split_at(split);
        if number.is_empty() || suffix.chars().any(|c| !c.is_ascii_alphabetic()) {
            return Err(format!("invalid size: {value}"));
        }

        let multiplier = match suffix.to_ascii_lowercase().as_str() {
            "" | "b" => 1_u128,
            "k" | "kb" | "kib" => 1024_u128,
            "m" | "mb" | "mib" => 1024_u128.pow(2),
            "g" | "gb" | "gib" => 1024_u128.pow(3),
            "t" | "tb" | "tib" => 1024_u128.pow(4),
            _ => return Err(format!("unsupported size suffix: {suffix}")),
        };

        let mut parts = number.split('.');
        let whole = parts
            .next()
            .ok_or_else(|| format!("invalid size: {value}"))?;
        let fraction = parts.next();
        if parts.next().is_some()
            || whole.is_empty()
            || !whole.chars().all(|c| c.is_ascii_digit())
            || fraction.is_some_and(|v| v.is_empty() || !v.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(format!("invalid size: {value}"));
        }

        let whole: u128 = whole
            .parse()
            .map_err(|_| format!("size is too large: {value}"))?;
        let mut bytes = whole
            .checked_mul(multiplier)
            .ok_or_else(|| format!("size is too large: {value}"))?;
        if let Some(fraction) = fraction {
            let scale = 10_u128
                .checked_pow(fraction.len() as u32)
                .ok_or_else(|| format!("size is too precise: {value}"))?;
            let fraction: u128 = fraction
                .parse()
                .map_err(|_| format!("invalid size: {value}"))?;
            bytes = bytes
                .checked_add(
                    fraction
                        .checked_mul(multiplier)
                        .ok_or_else(|| format!("size is too large: {value}"))?
                        / scale,
                )
                .ok_or_else(|| format!("size is too large: {value}"))?;
        }

        if bytes == 0 {
            return Err("size must be greater than zero".into());
        }
        if bytes > u64::MAX as u128 {
            return Err(format!("size is too large: {value}"));
        }
        Ok(Self(bytes as u64))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestartInterval(pub Duration);

impl FromStr for RestartInterval {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        let split = value
            .find(|c: char| c.is_ascii_alphabetic())
            .ok_or_else(|| format!("interval must include a unit: {value}"))?;
        let (number, suffix) = value.split_at(split);
        let multiplier = match suffix.to_ascii_lowercase().as_str() {
            "ms" => 1_000_000_u128,
            "s" => 1_000_000_000_u128,
            "m" => 60 * 1_000_000_000_u128,
            "h" => 60 * 60 * 1_000_000_000_u128,
            _ => return Err(format!("unsupported interval unit: {suffix}")),
        };
        let mut parts = number.split('.');
        let whole = parts
            .next()
            .filter(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| format!("invalid interval: {value}"))?;
        let fraction = parts.next();
        if parts.next().is_some()
            || fraction
                .is_some_and(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(format!("invalid interval: {value}"));
        }
        let whole: u128 = whole
            .parse()
            .map_err(|_| format!("interval is too large: {value}"))?;
        let mut nanos = whole
            .checked_mul(multiplier)
            .ok_or_else(|| format!("interval is too large: {value}"))?;
        if let Some(fraction) = fraction {
            let scale = 10_u128
                .checked_pow(fraction.len() as u32)
                .ok_or_else(|| format!("interval is too precise: {value}"))?;
            let fraction: u128 = fraction
                .parse()
                .map_err(|_| format!("invalid interval: {value}"))?;
            nanos = nanos
                .checked_add(
                    fraction
                        .checked_mul(multiplier)
                        .ok_or_else(|| format!("interval is too large: {value}"))?
                        / scale,
                )
                .ok_or_else(|| format!("interval is too large: {value}"))?;
        }
        if nanos > u64::MAX as u128 {
            return Err(format!("interval is too large: {value}"));
        }
        Ok(Self(Duration::from_nanos(nanos as u64)))
    }
}

#[derive(Parser, Debug)]
#[command(version, author, about = "Rotate stdin or child-process logs")]
pub struct Args {
    /// TOML configuration file
    #[arg(short, long, env = "LOG_ROTATE_CONFIG")]
    config: Option<PathBuf>,

    /// Common output file used by streams without an explicit output
    #[arg(short, long, env = "LOG_ROTATE_OUTPUT")]
    output: Option<PathBuf>,

    /// Output file for the child process stdout
    #[arg(long, env = "LOG_ROTATE_STDOUT_OUTPUT")]
    stdout_output: Option<PathBuf>,

    /// Output file for the child process stderr
    #[arg(long, env = "LOG_ROTATE_STDERR_OUTPUT")]
    stderr_output: Option<PathBuf>,

    /// Rotation mode
    #[arg(short = 'm', long, env = "LOG_ROTATE_CUT_MODE")]
    cut_mode: Option<CutMode>,

    /// Days to retain archives; 0 disables automatic cleanup
    #[arg(short = 'k', long, env = "LOG_KEEP_DAYS")]
    keep_days: Option<i64>,

    /// Maximum active-file size, e.g. 16MB or 1.5GiB
    #[arg(short = 's', long, env = "LOG_ROTATE_FILE_SIZE")]
    file_size: Option<ByteSize>,

    /// Compress rotated files with gzip
    #[arg(short = 'z', long, env = "LOG_ROTATE_COMPRESS", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    compress: Option<bool>,

    /// Prefix every log line with local time
    #[arg(long, env = "LOG_ROTATE_TIMESTAMP_PREFIX", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    timestamp_prefix: Option<bool>,

    /// Prefix stdout/stderr with [O]/[E] when they share one output
    #[arg(long, env = "LOG_ROTATE_STREAM_LABEL", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    stream_label: Option<bool>,

    /// Enable diagnostic messages
    #[arg(long, env = "LOG_ROTATE_DEBUG", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    debug: Option<bool>,

    /// Restart a child after an abnormal exit
    #[arg(long, env = "LOG_ROTATE_RESTART", num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    restart: Option<bool>,

    /// Maximum number of restarts after abnormal exits; omit for unlimited
    #[arg(long, env = "LOG_ROTATE_RESTART_COUNT")]
    restart_count: Option<u32>,

    /// Delay between restarts, e.g. 500ms, 2s, or 1m
    #[arg(long, env = "LOG_ROTATE_RESTART_INTERVAL")]
    restart_interval: Option<RestartInterval>,

    /// Command and arguments. Use `-- command --arg` for commands with options.
    #[arg(
        value_name = "COMMAND",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    command: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub output: PathBuf,
    pub stdout_output: Option<PathBuf>,
    pub stderr_output: Option<PathBuf>,
    pub cut_mode: CutMode,
    pub keep_days: i64,
    pub file_size: u64,
    pub compress: bool,
    pub timestamp_prefix: bool,
    pub stream_label: bool,
    pub debug: bool,
    pub restart: bool,
    pub restart_count: Option<u32>,
    pub restart_interval: Duration,
    pub command: Vec<String>,
}

impl RuntimeConfig {
    pub fn is_process_mode(&self) -> bool {
        !self.command.is_empty()
    }

    pub fn stdout_path(&self) -> PathBuf {
        self.stdout_output
            .clone()
            .unwrap_or_else(|| self.output.clone())
    }

    pub fn stderr_path(&self) -> PathBuf {
        self.stderr_output
            .clone()
            .unwrap_or_else(|| self.output.clone())
    }
}

#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn string(table: &Table, key: &str) -> Result<Option<String>, ConfigError> {
    table
        .get(key)
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| ConfigError(format!("config `{key}` must be a string")))
        })
        .transpose()
}

fn boolean(table: &Table, key: &str) -> Result<Option<bool>, ConfigError> {
    table
        .get(key)
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| ConfigError(format!("config `{key}` must be a boolean")))
        })
        .transpose()
}

fn unsigned(table: &Table, key: &str) -> Result<Option<u32>, ConfigError> {
    table
        .get(key)
        .map(|v| {
            v.as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    ConfigError(format!("config `{key}` must be a non-negative integer"))
                })
        })
        .transpose()
}

fn configured_interval(table: &Table) -> Result<Option<RestartInterval>, ConfigError> {
    let Some(value) = table.get("restart_interval") else {
        return Ok(None);
    };
    if let Some(value) = value.as_integer() {
        return u64::try_from(value)
            .map(|milliseconds| RestartInterval(Duration::from_millis(milliseconds)))
            .map(Some)
            .map_err(|_| ConfigError("config `restart_interval` is too large".into()));
    }
    if let Some(value) = value.as_str() {
        return value
            .parse()
            .map(Some)
            .map_err(|e: String| ConfigError(format!("invalid config `restart_interval`: {e}")));
    }
    Err(ConfigError(
        "config `restart_interval` must be milliseconds or a duration string".into(),
    ))
}

fn integer(table: &Table, key: &str) -> Result<Option<i64>, ConfigError> {
    table
        .get(key)
        .map(|v| {
            v.as_integer()
                .ok_or_else(|| ConfigError(format!("config `{key}` must be an integer")))
        })
        .transpose()
}

fn configured_size(table: &Table) -> Result<Option<ByteSize>, ConfigError> {
    let Some(value) = table.get("file_size") else {
        return Ok(None);
    };
    if let Some(value) = value.as_integer() {
        return u64::try_from(value)
            .ok()
            .filter(|v| *v > 0)
            .map(ByteSize)
            .ok_or_else(|| ConfigError("config `file_size` must be greater than zero".into()))
            .map(Some);
    }
    if let Some(value) = value.as_str() {
        return value
            .parse()
            .map(Some)
            .map_err(|e: String| ConfigError(format!("invalid config `file_size`: {e}")));
    }
    Err(ConfigError(
        "config `file_size` must be an integer or size string".into(),
    ))
}

pub fn load() -> Result<RuntimeConfig, ConfigError> {
    resolve(Args::parse())
}

fn resolve(args: Args) -> Result<RuntimeConfig, ConfigError> {
    let table = if let Some(path) = &args.config {
        let contents = fs::read_to_string(path).map_err(|e| {
            ConfigError(format!(
                "failed to read config file `{}`: {e}",
                path.display()
            ))
        })?;
        if contents.trim().is_empty() {
            return Err(ConfigError("config file is empty".into()));
        }
        contents
            .parse::<Table>()
            .map_err(|e| ConfigError(format!("failed to parse config file: {e}")))?
    } else {
        Table::new()
    };

    let keep_days = match (table.get("keep_days"), table.get("keep_num")) {
        (Some(_), Some(_)) => {
            return Err(ConfigError(
                "config cannot contain both `keep_days` and legacy `keep_num`".into(),
            ))
        }
        (Some(_), None) => integer(&table, "keep_days")?,
        (None, Some(_)) => integer(&table, "keep_num")?,
        (None, None) => None,
    };

    let config_command = table
        .get("exec")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| ConfigError("config `exec` must be an array of strings".into()))?
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_owned).ok_or_else(|| {
                        ConfigError("config `exec` must be an array of strings".into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    let command = if args.command.is_empty() {
        config_command
    } else {
        args.command
    };
    let stdout_output = args
        .stdout_output
        .or(string(&table, "stdout_output")?.map(PathBuf::from));
    let stderr_output = args
        .stderr_output
        .or(string(&table, "stderr_output")?.map(PathBuf::from));
    if command.is_empty() && (stdout_output.is_some() || stderr_output.is_some()) {
        return Err(ConfigError(
            "`stdout_output` and `stderr_output` require a child command".into(),
        ));
    }

    let keep_days = args.keep_days.or(keep_days).unwrap_or(0);
    if keep_days < 0 {
        return Err(ConfigError("`keep_days` cannot be negative".into()));
    }

    let cut_mode = if let Some(mode) = args.cut_mode {
        mode
    } else if let Some(mode) = string(&table, "cut_mode")? {
        CutMode::from_str(&mode, true)
            .map_err(|_| ConfigError(format!("invalid config `cut_mode`: {mode}")))?
    } else {
        CutMode::Size
    };

    Ok(RuntimeConfig {
        output: args
            .output
            .or(string(&table, "output")?.map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT)),
        stdout_output,
        stderr_output,
        cut_mode,
        keep_days,
        file_size: args
            .file_size
            .or(configured_size(&table)?)
            .unwrap_or(ByteSize(DEFAULT_FILE_SIZE))
            .0,
        compress: args
            .compress
            .or(boolean(&table, "compress")?)
            .unwrap_or(false),
        timestamp_prefix: args
            .timestamp_prefix
            .or(boolean(&table, "timestamp_prefix")?)
            .unwrap_or(false),
        stream_label: args
            .stream_label
            .or(boolean(&table, "stream_label")?)
            .unwrap_or(false),
        debug: args.debug.or(boolean(&table, "debug")?).unwrap_or(false),
        restart: args
            .restart
            .or(boolean(&table, "restart")?)
            .unwrap_or(false),
        restart_count: args.restart_count.or(unsigned(&table, "restart_count")?),
        restart_interval: args
            .restart_interval
            .or(configured_interval(&table)?)
            .unwrap_or(RestartInterval(Duration::from_secs(1)))
            .0,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::{ByteSize, RestartInterval};
    use std::time::Duration;

    #[test]
    fn parses_human_sizes_using_binary_multipliers() {
        assert_eq!("512".parse::<ByteSize>().unwrap().0, 512);
        assert_eq!("1KB".parse::<ByteSize>().unwrap().0, 1024);
        assert_eq!("1.5mb".parse::<ByteSize>().unwrap().0, 1_572_864);
        assert_eq!("2GiB".parse::<ByteSize>().unwrap().0, 2_147_483_648);
        assert_eq!("0.1KB".parse::<ByteSize>().unwrap().0, 102);
    }

    #[test]
    fn rejects_invalid_human_sizes() {
        for value in ["", "0", "-1MB", "1.2.3MB", "1XB", ".5MB"] {
            assert!(value.parse::<ByteSize>().is_err(), "accepted {value}");
        }
    }

    #[test]
    fn parses_restart_intervals() {
        assert_eq!(
            "500ms".parse::<RestartInterval>().unwrap().0,
            Duration::from_millis(500)
        );
        assert_eq!(
            "1.5s".parse::<RestartInterval>().unwrap().0,
            Duration::from_millis(1500)
        );
        assert!("10".parse::<RestartInterval>().is_err());
    }
}
