//! The error type every command reports through.

use std::fmt;

/// A failure, printed as `workforest: <message>` before exiting with status 1.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Describe what was being attempted when an I/O operation failed.
pub trait Context<T> {
    fn context(self, what: impl fmt::Display) -> Result<T>;
}

impl<T> Context<T> for std::io::Result<T> {
    fn context(self, what: impl fmt::Display) -> Result<T> {
        self.map_err(|err| Error::new(format!("{what}: {err}")))
    }
}

/// Return early with an [`Error`] built from a format string.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::error::Error::new(format!($($arg)*)))
    };
}
pub(crate) use bail;
