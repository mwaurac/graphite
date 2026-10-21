//! Graphite: a small property graph database with its own query language.
//!
//! The implementation is built in layers. This slice adds the runtime value
//! model and the in-memory property graph with label and adjacency indexes.

pub mod error;
pub mod graph;
pub mod value;

pub use error::{Error, Result};
pub use graph::{Edge, EdgeId, Graph, Node, NodeId};
pub use value::Value;
