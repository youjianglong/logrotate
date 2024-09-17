pub fn trim_file_src(mut src: &str) -> &str {
    if src.starts_with("src/") || src.starts_with("src\\") {
        src = &src[4..];
    }
    src
}

pub static mut DEBUG: bool = false;

pub fn set_debug(debug: bool) {
    unsafe {
        DEBUG = debug;
    }
}

#[macro_export]
macro_rules! log {
    ($fmt:expr, $($arg:tt)*) => {
      if unsafe {crate::utils::DEBUG} {
        println!("[{}:{}] - {}", crate::utils::trim_file_src(file!()), line!(), format!($fmt, $($arg)*))
      }
    };
    ($fmt:expr) => {
      if unsafe {crate::utils::DEBUG} {
        println!("[{}:{}] - {}", crate::utils::trim_file_src(file!()), line!(), $fmt)
      }
    };
    () => {
      if unsafe {crate::utils::DEBUG} {
        println!("[{}:{}] -", crate::utils::trim_file_src(file!()), line!())
      }
    }
}

pub struct Lines<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lines<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Lines { data, pos: 0 }
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.data.len() {
            return None;
        }

        let start = self.pos;
        let end = self.data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|pos| pos + start + 1)
            .unwrap_or(self.data.len());

        self.pos = end;
        Some(&self.data[start..self.pos])
    }
}
