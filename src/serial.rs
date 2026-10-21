use std::collections::BTreeMap;

use crate::graph::{Graph, NodeId};
use crate::value::props_suffix;

/// Render the whole graph as a Graphite script.
///
/// The script format is deliberately the same language used by the database,
/// so loading is just executing the statements in order. Nodes are emitted
/// before relationships, which guarantees that relationship endpoints exist.
pub fn graph_to_script(graph: &Graph) -> String {
    let mut out = String::from("// Graphite snapshot\n");

    let mut variables: BTreeMap<NodeId, String> = BTreeMap::new();
    for (index, id) in graph.node_ids().iter().enumerate() {
        variables.insert(*id, format!("n{}", index + 1));
    }

    let mut patterns = Vec::new();

    for (id, variable) in &variables {
        let node = graph.node(*id).expect("node id came from the graph");
        let mut pattern = format!("({variable}");
        for label in &node.labels {
            pattern.push(':');
            pattern.push_str(label);
        }
        pattern.push_str(&props_suffix(&node.props));
        pattern.push(')');
        patterns.push(pattern);
    }

    for id in graph.edge_ids() {
        let edge = graph.edge(id).expect("edge id came from the graph");
        let from = &variables[&edge.from];
        let to = &variables[&edge.to];
        patterns.push(format!(
            "({from})-[:{}{}]->({to})",
            edge.rel_type,
            props_suffix(&edge.props)
        ));
    }

    if patterns.is_empty() {
        return out;
    }

    // Everything goes in one statement so node variables are shared between
    // node patterns and the relationships that refer to them.
    out.push_str("CREATE\n");
    for (index, pattern) in patterns.iter().enumerate() {
        out.push_str("  ");
        out.push_str(pattern);
        if index + 1 < patterns.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(";\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Database;
    use crate::value::Value;

    #[test]
    fn snapshot_round_trips_through_the_query_language() {
        let mut db = Database::new();
        db.execute(
            r#"CREATE (a:Person {name: "Alice", meta: {active: true, tags: ["x", "y"]}})
                      -[:KNOWS {since: 2019}]->(b:Person {name: "Bob"})"#,
        )
        .unwrap();

        let script = graph_to_script(db.graph());
        let restored = Database::from_script(&script).unwrap();

        assert_eq!(restored.graph().node_count(), 2);
        assert_eq!(restored.graph().edge_count(), 1);
        let alice = restored.graph().node(1).unwrap();
        assert_eq!(
            alice.props.get("name"),
            Some(&Value::String("Alice".into()))
        );
        assert_eq!(
            alice.props.get("meta"),
            Some(&Value::Map(BTreeMap::from([
                ("active".to_string(), Value::Bool(true)),
                (
                    "tags".to_string(),
                    Value::List(vec![Value::String("x".into()), Value::String("y".into())])
                ),
            ])))
        );
    }

    #[test]
    fn empty_graph_snapshot_loads_as_empty_database() {
        let db = Database::new();
        let restored = Database::from_script(&graph_to_script(db.graph())).unwrap();
        assert_eq!(restored.graph().node_count(), 0);
    }
}
