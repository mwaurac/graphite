use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::error::{Error, Result};
use crate::value::Value;

pub type NodeId = u64;
pub type EdgeId = u64;

/// A node in the graph. Labels are stored in insertion order; properties are
/// sorted by key for deterministic output.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub props: BTreeMap<String, Value>,
}

impl Node {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|existing| existing == label)
    }
}

/// A directed, typed relationship between two nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub rel_type: String,
    pub props: BTreeMap<String, Value>,
}

/// An in-memory property graph.
///
/// The representation is intentionally simple for the first milestone:
/// ordered maps for stable iteration plus label and adjacency indexes so
/// lookups do not scan the whole graph.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<EdgeId, Edge>,
    next_node_id: NodeId,
    next_edge_id: EdgeId,
    labels: HashMap<String, BTreeSet<NodeId>>,
    outgoing: HashMap<NodeId, BTreeSet<EdgeId>>,
    incoming: HashMap<NodeId, BTreeSet<EdgeId>>,
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            next_node_id: 1,
            next_edge_id: 1,
            ..Graph::default()
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    pub fn edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(&id)
    }

    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    pub fn edge_ids(&self) -> Vec<EdgeId> {
        self.edges.keys().copied().collect()
    }

    pub fn nodes_with_label(&self, label: &str) -> Vec<NodeId> {
        self.labels
            .get(label)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn outgoing_ids(&self, id: NodeId) -> Vec<EdgeId> {
        self.outgoing
            .get(&id)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn incoming_ids(&self, id: NodeId) -> Vec<EdgeId> {
        self.incoming
            .get(&id)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn incident_ids(&self, id: NodeId) -> Vec<EdgeId> {
        let mut ids = self.outgoing_ids(id);
        if let Some(incoming) = self.incoming.get(&id) {
            for edge_id in incoming {
                if !ids.contains(edge_id) {
                    ids.push(*edge_id);
                }
            }
        }
        ids.sort_unstable();
        ids
    }

    pub fn add_node(&mut self, labels: Vec<String>, props: BTreeMap<String, Value>) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;

        let mut unique_labels = Vec::with_capacity(labels.len());
        for label in labels {
            if !unique_labels.contains(&label) {
                unique_labels.push(label);
            }
        }

        for label in &unique_labels {
            self.labels.entry(label.clone()).or_default().insert(id);
        }

        self.nodes.insert(
            id,
            Node {
                id,
                labels: unique_labels,
                props,
            },
        );
        id
    }

    pub fn add_edge(
        &mut self,
        from: NodeId,
        to: NodeId,
        rel_type: String,
        props: BTreeMap<String, Value>,
    ) -> Result<EdgeId> {
        if !self.nodes.contains_key(&from) {
            return Err(Error::new(format!(
                "cannot create relationship: source node #{from} does not exist"
            )));
        }
        if !self.nodes.contains_key(&to) {
            return Err(Error::new(format!(
                "cannot create relationship: target node #{to} does not exist"
            )));
        }

        let id = self.next_edge_id;
        self.next_edge_id += 1;

        self.edges.insert(
            id,
            Edge {
                id,
                from,
                to,
                rel_type,
                props,
            },
        );
        self.outgoing.entry(from).or_default().insert(id);
        self.incoming.entry(to).or_default().insert(id);
        Ok(id)
    }

    /// Set a property on a node or relationship. Returns true when the
    /// property did not exist before.
    pub fn set_node_property(&mut self, id: NodeId, key: String, value: Value) -> Result<bool> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or_else(|| Error::new(format!("node #{id} no longer exists")))?;
        Ok(node.props.insert(key, value).is_none())
    }

    pub fn set_edge_property(&mut self, id: EdgeId, key: String, value: Value) -> Result<bool> {
        let edge = self
            .edges
            .get_mut(&id)
            .ok_or_else(|| Error::new(format!("relationship #{id} no longer exists")))?;
        Ok(edge.props.insert(key, value).is_none())
    }

    pub fn remove_edge(&mut self, id: EdgeId) -> Option<Edge> {
        let edge = self.edges.remove(&id)?;
        if let Some(ids) = self.outgoing.get_mut(&edge.from) {
            ids.remove(&id);
            if ids.is_empty() {
                self.outgoing.remove(&edge.from);
            }
        }
        if let Some(ids) = self.incoming.get_mut(&edge.to) {
            ids.remove(&id);
            if ids.is_empty() {
                self.incoming.remove(&edge.to);
            }
        }
        Some(edge)
    }

    /// Remove a node and return the number of relationships that were removed
    /// with it. Without `detach`, a node that still has relationships is not
    /// removed and an error is returned.
    pub fn remove_node(&mut self, id: NodeId, detach: bool) -> Result<usize> {
        if !self.nodes.contains_key(&id) {
            return Err(Error::new(format!("node #{id} no longer exists")));
        }

        let incident = self.incident_ids(id);
        if !incident.is_empty() && !detach {
            return Err(Error::new(format!(
                "cannot delete node #{id}: it still has {} relationship(s); use DETACH DELETE to remove them too",
                incident.len()
            )));
        }

        let removed_edges = incident.len();
        for edge_id in incident {
            self.remove_edge(edge_id);
        }

        let node = self.nodes.remove(&id).expect("node checked above");
        for label in &node.labels {
            if let Some(ids) = self.labels.get_mut(label) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.labels.remove(label);
                }
            }
        }

        Ok(removed_edges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn creates_and_indexes_nodes_and_edges() {
        let mut graph = Graph::new();
        let alice = graph.add_node(
            vec!["Person".into()],
            props(&[("name", Value::String("Alice".into()))]),
        );
        let bob = graph.add_node(
            vec!["Person".into()],
            props(&[("name", Value::String("Bob".into()))]),
        );
        graph
            .add_edge(alice, bob, "KNOWS".into(), BTreeMap::new())
            .unwrap();

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.nodes_with_label("Person"), vec![alice, bob]);
        assert_eq!(graph.outgoing_ids(alice).len(), 1);
        assert_eq!(graph.incoming_ids(bob).len(), 1);
        assert_eq!(graph.incident_ids(alice).len(), 1);
    }

    #[test]
    fn refuses_to_delete_connected_node_without_detach() {
        let mut graph = Graph::new();
        let a = graph.add_node(vec![], BTreeMap::new());
        let b = graph.add_node(vec![], BTreeMap::new());
        graph
            .add_edge(a, b, "KNOWS".into(), BTreeMap::new())
            .unwrap();

        assert!(graph.remove_node(a, false).is_err());
        assert_eq!(graph.node_count(), 2);
        graph.remove_node(a, true).unwrap();
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 0);
    }
}
