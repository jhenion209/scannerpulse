//! Lines for the journal. The `<n>` prefix gives each line its priority
//! (sd-daemon(3)): 3 error, 4 warning, 6 information, 7 debug.

use std::fmt::Display;

pub fn error(message: impl Display) {
    eprintln!("<3>{message}");
}

pub fn warn(message: impl Display) {
    eprintln!("<4>{message}");
}

pub fn info(message: impl Display) {
    eprintln!("<6>{message}");
}

pub fn debug(message: impl Display) {
    eprintln!("<7>{message}");
}
