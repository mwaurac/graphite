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
    journal: Option<UndoJournal>,
}

/// Inverse operations recorded while a transaction is active.
#[derive(Clone, Debug)]
struct UndoJournal {
    entries: Vec<UndoEntry>,
    next_node_id: NodeId,
    next_edge_id: EdgeId,
}

#[derive(Clone, Debug)]
enum UndoEntry {
    CreatedNode(NodeId),
    CreatedEdge(EdgeId),
    SetNodeProperty {
        id: NodeId,
        key: String,
        previous: Option<Value>,
    },
    SetEdgeProperty {
        id: EdgeId,
        key: String,
        previous: Option<Value>,
    },
    RemovedEdge(Edge),
    RemovedNode(Node),
}

impl Graph {
    pub fn new() -> Self {
        Graph {
            next_node_id: 1,
            next_edge_id: 1,
            ..Graph::default()
        }
    }

    /// Start recording inverse operations for a transaction. Nested journals
    /// are not supported; the first call wins.
    pub(crate) fn begin_undo(&mut self) {
        if self.journal.is_none() {
            self.journal = Some(UndoJournal {
                entries: Vec::new(),
                next_node_id: self.next_node_id,
                next_edge_id: self.next_edge_id,
            });
        }
    }

    /// Keep all changes and stop recording undo entries.
    pub(crate) fn commit_undo(&mut self) {
        self.journal = None;
    }

    /// Revert every change recorded since [`Graph::begin_undo`].
    pub(crate) fn rollback_undo(&mut self) {
        let Some(journal) = self.journal.take() else {
            return;
        };

        for entry in journal.entries.into_iter().rev() {
            self.revert(entry);
        }
        self.next_node_id = journal.next_node_id;
        self.next_edge_id = journal.next_edge_id;
    }

    pub(crate) fn has_undo(&self) -> bool {
        self.journal.is_some()
    }

    fn record_undo(&mut self, entry: UndoEntry) {
        if let Some(journal) = &mut self.journal {
            journal.entries.push(entry);
        }
    }

    fn revert(&mut self, entry: UndoEntry) {
        match entry {
            UndoEntry::CreatedNode(id) => {
                if let Some(node) = self.nodes.remove(&id) {
                    self.unindex_node(&node);
                }
            }
            UndoEntry::CreatedEdge(id) => {
                if let Some(edge) = self.edges.remove(&id) {
                    self.unlink_edge(&edge);
                }
            }
            UndoEntry::SetNodeProperty { id, key, previous } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    match previous {
                        Some(value) => {
                            node.props.insert(key, value);
                        }
                        None => {
                            node.props.remove(&key);
                        }
                    }
                }
            }
            UndoEntry::SetEdgeProperty { id, key, previous } => {
                if let Some(edge) = self.edges.get_mut(&id) {
                    match previous {
                        Some(value) => {
                            edge.props.insert(key, value);
                        }
                        None => {
                            edge.props.remove(&key);
                        }
                    }
                }
            }
            UndoEntry::RemovedEdge(edge) => {
                let id = edge.id;
                let from = edge.from;
                let to = edge.to;
                self.edges.insert(id, edge);
                self.outgoing.entry(from).or_default().insert(id);
                self.incoming.entry(to).or_default().insert(id);
            }
            UndoEntry::RemovedNode(node) => {
                for label in &node.labels {
                    self.labels
                        .entry(label.clone())
                        .or_default()
                        .insert(node.id);
                }
                self.nodes.insert(node.id, node);
            }
        }
    }

    fn unindex_node(&mut self, node: &Node) {
        for label in &node.labels {
            if let Some(ids) = self.labels.get_mut(label) {
                ids.remove(&node.id);
                if ids.is_empty() {
                    self.labels.remove(label);
                }
            }
        }
    }

    fn unlink_edge(&mut self, edge: &Edge) {
        if let Some(ids) = self.outgoing.get_mut(&edge.from) {
            ids.remove(&edge.id);
            if ids.is_empty() {
                self.outgoing.remove(&edge.from);
            }
        }
        if let Some(ids) = self.incoming.get_mut(&edge.to) {
            ids.remove(&edge.id);
            if ids.is_empty() {
                self.incoming.remove(&edge.to);
            }
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
        self.record_undo(UndoEntry::CreatedNode(id));
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
        self.record_undo(UndoEntry::CreatedEdge(id));
        Ok(id)
    }

    /// Set a property on a node or relationship. Returns true when the
    /// property did not exist before.
    pub fn set_node_property(&mut self, id: NodeId, key: String, value: Value) -> Result<bool> {
        let (previous, inserted) = {
            let node = self
                .nodes
                .get_mut(&id)
                .ok_or_else(|| Error::new(format!("node #{id} no longer exists")))?;
            let previous = node.props.get(&key).cloned();
            let inserted = node.props.insert(key.clone(), value).is_none();
            (previous, inserted)
        };
        self.record_undo(UndoEntry::SetNodeProperty { id, key, previous });
        Ok(inserted)
    }

    pub fn set_edge_property(&mut self, id: EdgeId, key: String, value: Value) -> Result<bool> {
        let (previous, inserted) = {
            let edge = self
                .edges
                .get_mut(&id)
                .ok_or_else(|| Error::new(format!("relationship #{id} no longer exists")))?;
            let previous = edge.props.get(&key).cloned();
            let inserted = edge.props.insert(key.clone(), value).is_none();
            (previous, inserted)
        };
        self.record_undo(UndoEntry::SetEdgeProperty { id, key, previous });
        Ok(inserted)
    }

    pub fn remove_edge(&mut self, id: EdgeId) -> Option<Edge> {
        let edge = self.edges.remove(&id)?;
        self.unlink_edge(&edge);
        self.record_undo(UndoEntry::RemovedEdge(edge.clone()));
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
        self.unindex_node(&node);
        self.record_undo(UndoEntry::RemovedNode(node));

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
