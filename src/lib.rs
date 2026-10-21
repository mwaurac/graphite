//! Graphite: a small property graph database with its own query language.
//!
//! ```
//! use graphite::Database;
//!
//! let mut db = Database::new();
//! db.execute(r#"CREATE (a:Person {name: "Alice"})-[:KNOWS]->(b:Person {name: "Bob"})"#)
//!     .unwrap();
//!
//! let results = db
//!     .execute(r#"MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name AS from, b.name AS to"#)
//!     .unwrap();
//! assert_eq!(results[0].rows.len(), 1);
//! ```

pub mod ast;
pub mod error;
pub mod executor;
pub mod graph;
pub mod lexer;
pub mod parser;
pub mod value;

pub use error::{Error, Result};
pub use executor::{Database, QueryResult, Stats};
pub use graph::{Edge, EdgeId, Graph, Node, NodeId};
pub use value::Value;

/// A small sample graph used by `:demo` in the REPL and by the examples.
pub const DEMO_QUERY: &str = r#"
CREATE
  (alice:Person {name: "Alice", age: 30, city: "Nairobi"}),
  (bob:Person {name: "Bob", age: 27, city: "Mombasa"}),
  (carol:Person {name: "Carol", age: 35, city: "Nairobi"}),
  (dave:Person {name: "Dave", age: 41, city: "Kisumu"}),
  (acme:Company {name: "Acme", founded: 2015}),
  (orbit:Company {name: "Orbit", founded: 2020}),
  (alice)-[:KNOWS {since: 2019}]->(bob),
  (bob)-[:KNOWS {since: 2021}]->(carol),
  (carol)-[:KNOWS {since: 2017}]->(dave),
  (alice)-[:WORKS_AT {role: "Engineer"}]->(acme),
  (bob)-[:WORKS_AT {role: "Designer"}]->(acme),
  (carol)-[:WORKS_AT {role: "CTO"}]->(orbit),
  (dave)-[:WORKS_AT {role: "Engineer"}]->(orbit);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_script_with_multiple_statements() {
        let mut db = Database::new();
        let results = db
            .execute(
                r#"
                CREATE (:Person {name: "Alice"});
                CREATE (:Person {name: "Bob"});
                MATCH (n:Person) RETURN count(*) AS total;
                "#,
            )
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].stats.nodes_created, 1);
        assert_eq!(results[1].stats.nodes_created, 1);
        assert_eq!(results[2].rows[0][0], Value::Int(2));
    }

    #[test]
    fn demo_query_loads() {
        let mut db = Database::new();
        db.execute(DEMO_QUERY).unwrap();
        assert_eq!(db.graph().node_count(), 6);
        assert_eq!(db.graph().edge_count(), 7);
    }
}
