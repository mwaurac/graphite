use std::fmt;

/// A query, parse, or execution error.
///
/// Errors carry an optional source position so the CLI can point at the
/// offending part of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
            line: None,
            column: None,
        }
    }

    pub fn at(message: impl Into<String>, line: usize, column: usize) -> Self {
        Error {
            message: message.into(),
            line: Some(line),
            column: Some(column),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(f, "{} (line {}, column {})", self.message, line, column)
            }
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
