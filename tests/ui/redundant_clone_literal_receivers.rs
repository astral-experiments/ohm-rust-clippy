//@check-pass
#![warn(clippy::redundant_clone)]

macro_rules! identity {
    ($expression:expr) => {
        $expression
    };
}

fn main() {
    let _ = "literal".to_owned();
    let _ = "literal".to_string();
    let _ = b"bytes".to_owned();
    let _ = c"ffi".to_owned();
    let _ = 42_u8.to_string();
    let _ = identity!("macro literal").to_owned();
    let _ = || "closure literal".to_string();
}
