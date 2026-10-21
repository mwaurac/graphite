//! Graphite: a small property graph database with its own query language.
//!
//! The implementation is built in layers. This slice adds the lexer that turns
//! query text into tokens with source positions.

pub mod error;
pub mod graph;
pub mod lexer;
pub mod value;

pub use error::{Error, Result};
pub use graph::{Edge, EdgeId, Graph, Node, NodeId};
pub use value::Value;
