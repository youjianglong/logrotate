use std::sync::atomic::{AtomicBool, Ordering};

pub fn trim_file_src(mut src: &str) -> &str {
    if src.starts_with("src/") || src.starts_with("src\\") {
        src = &src[4..];
    }
    src
}

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(debug: bool) {
    DEBUG.store(debug, Ordering::Relaxed);
}

pub fn debug_enabled() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! log {
    ($fmt:expr, $($arg:tt)*) => {
        if $crate::utils::debug_enabled() {
            eprintln!(
                "[{}:{}] - {}",
                $crate::utils::trim_file_src(file!()),
                line!(),
                format!($fmt, $($arg)*)
            )
        }
    };
    ($fmt:expr) => {
        if $crate::utils::debug_enabled() {
            eprintln!(
                "[{}:{}] - {}",
                $crate::utils::trim_file_src(file!()),
                line!(),
                format!($fmt)
            )
        }
    };
    () => {
        if $crate::utils::debug_enabled() {
            eprintln!(
                "[{}:{}] -",
                $crate::utils::trim_file_src(file!()),
                line!()
            )
        }
    };
}
