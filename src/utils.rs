pub fn trim_file_src(mut src: &str) -> &str {
    if src.starts_with("src/") || src.starts_with("src\\") {
        src = &src[4..];
    }
    src
}

#[macro_export]
macro_rules! log {
    ($fmt:expr, $($arg:tt)*) => {
      println!("[{}:{}] - {}", crate::utils::trim_file_src(file!()), line!(), format!($fmt, $($arg)*))
    };
    ($fmt:expr) => {
      println!("[{}:{}] - {}", crate::utils::trim_file_src(file!()), line!(), $fmt)
    };
    () => {
      println!("[{}:{}] -", crate::utils::trim_file_src(file!()), line!())
    }
}
