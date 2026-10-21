//! Graphite: a small property graph database with its own query language.
//!
//! The implementation is built in layers. This first slice defines the
//! positioned error type used by every later layer.

pub mod error;

pub use error::{Error, Result};
