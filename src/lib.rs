//! Graphite: a small property graph database with its own query language.
//!
//! The implementation is built in layers. This slice adds the AST and the
//! recursive-descent parser for `CREATE` and `MATCH` statements.

pub mod ast;
pub mod error;
pub mod graph;
pub mod lexer;
pub mod parser;
pub mod value;

pub use error::{Error, Result};
pub use graph::{Edge, EdgeId, Graph, Node, NodeId};
pub use value::Value;
