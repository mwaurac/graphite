use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::ast::{
    Assignment, BinaryOp, Direction, Expr, MatchAction, NodePattern, Pattern, RelChain, RelPattern,
    Return, Statement, UnaryOp,
};
use crate::error::{Error, Result};
use crate::graph::{Edge, EdgeId, Graph, Node, NodeId};
use crate::parser::parse_program;
use crate::storage::{self, Storage};
use crate::value::Value;

#[derive(Clone, Debug)]
enum Binding {
    Node(Node),
    Edge(Edge),
}

impl Binding {
    fn as_value(&self) -> Value {
        match self {
            Binding::Node(node) => Value::Node(Box::new(node.clone())),
            Binding::Edge(edge) => Value::Edge(Box::new(edge.clone())),
        }
    }
}

type Row = BTreeMap<String, Binding>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub nodes_created: usize,
    pub edges_created: usize,
    pub nodes_deleted: usize,
    pub edges_deleted: usize,
    pub properties_set: usize,
}

impl Stats {
    pub fn merge(&mut self, other: &Stats) {
        self.nodes_created += other.nodes_created;
        self.edges_created += other.edges_created;
        self.nodes_deleted += other.nodes_deleted;
        self.edges_deleted += other.edges_deleted;
        self.properties_set += other.properties_set;
    }

    pub fn is_empty(&self) -> bool {
        *self == Stats::default()
    }

    pub fn summary(&self) -> String {
        let mut clauses = Vec::new();

        if self.nodes_created > 0 || self.edges_created > 0 {
            let mut items = Vec::new();
            if self.nodes_created > 0 {
                items.push(format!(
                    "{} {}",
                    self.nodes_created,
                    plural(self.nodes_created, "node", "nodes")
                ));
            }
            if self.edges_created > 0 {
                items.push(format!(
                    "{} {}",
                    self.edges_created,
                    plural(self.edges_created, "relationship", "relationships")
                ));
            }
            clauses.push(format!("created {}", items.join(" and ")));
        }

        if self.nodes_deleted > 0 || self.edges_deleted > 0 {
            let mut items = Vec::new();
            if self.nodes_deleted > 0 {
                items.push(format!(
                    "{} {}",
                    self.nodes_deleted,
                    plural(self.nodes_deleted, "node", "nodes")
                ));
            }
            if self.edges_deleted > 0 {
                items.push(format!(
                    "{} {}",
                    self.edges_deleted,
                    plural(self.edges_deleted, "relationship", "relationships")
                ));
            }
            clauses.push(format!("deleted {}", items.join(" and ")));
        }

        if self.properties_set > 0 {
            clauses.push(format!(
                "set {} {}",
                self.properties_set,
                plural(self.properties_set, "property", "properties")
            ));
        }

        if clauses.is_empty() {
            return String::new();
        }

        let mut sentence = clauses.join(", ");
        if let Some(first) = sentence.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        sentence.push('.');
        sentence
    }
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

/// The result of executing a single statement.
#[derive(Clone, Debug, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub stats: Stats,
}

struct ProjectedRow {
    values: Vec<Value>,
    source: Option<usize>,
    group: Option<Vec<usize>>,
}

impl QueryResult {
    pub fn summary(&self) -> String {
        self.stats.summary()
    }
}

/// A graph database instance.
///
/// Created with [`Database::new`], it lives entirely in memory. Created with
/// [`Database::open`], it records every committed transaction in a
/// write-ahead log and recovers from that log on startup.
#[derive(Debug)]
pub struct Database {
    graph: Graph,
    storage: Option<Storage>,
    last_sequence: u64,
}

impl Default for Database {
    fn default() -> Self {
        Self::new()
    }
}

impl Database {
    pub fn new() -> Self {
        Database {
            graph: Graph::new(),
            storage: None,
            last_sequence: 0,
        }
    }

    /// Open or create a database in a directory.
    ///
    /// Recovery loads `snapshot.gql`, replays valid WAL records after the
    /// snapshot sequence, then checkpoints so the WAL starts clean.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        // Take the directory lock before reading any state, so recovery cannot
        // race another process.
        let storage_handle = Storage::open(directory)?;
        let snapshot = storage::read_snapshot(directory)?;

        let mut db = match snapshot {
            Some((sequence, script)) => {
                let mut db = Database::from_script(&script)?;
                db.last_sequence = sequence;
                db
            }
            None => Database::new(),
        };

        let records = storage::read_wal(directory)?;
        let had_records = !records.is_empty();

        for (sequence, payload) in records {
            if sequence <= db.last_sequence {
                continue;
            }

            let source = String::from_utf8(payload).map_err(|error| {
                Error::new(format!(
                    "write-ahead log record {sequence} is not valid UTF-8: {error}"
                ))
            })?;

            let mut transaction = db.begin();
            transaction.execute(&source).map_err(|error| {
                Error::new(format!(
                    "cannot replay write-ahead log record {sequence}: {error}"
                ))
            })?;
            transaction.commit()?;
            db.last_sequence = sequence;
        }

        db.storage = Some(storage_handle);
        if had_records {
            db.checkpoint()?;
        }

        Ok(db)
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// The directory backing this database, if it is persistent.
    pub fn directory(&self) -> Option<&Path> {
        self.storage.as_ref().map(Storage::directory)
    }

    /// The sequence number of the last committed transaction.
    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    /// Start a transaction. Read-only transactions do not copy the graph;
    /// the first write in a transaction stages a copy for atomic commit.
    pub fn begin(&mut self) -> Transaction<'_> {
        Transaction {
            db: self,
            staged: None,
            sources: Vec::new(),
            failed: false,
        }
    }

    /// Build a database from a script, replacing nothing (the database is new).
    pub fn from_script(source: &str) -> Result<Self> {
        let statements = parse_program(source)?;
        let mut db = Database::new();
        if !statements.is_empty() {
            db.execute(source)?;
        }
        Ok(db)
    }

    /// Render the graph as a script that can be loaded with [`Database::from_script`].
    pub fn dump(&self) -> String {
        crate::serial::graph_to_script(&self.graph)
    }

    /// Force a durable snapshot and truncate the write-ahead log.
    ///
    /// This is a no-op for in-memory databases.
    pub fn checkpoint(&mut self) -> Result<()> {
        if let Some(storage) = self.storage.as_mut() {
            let script = crate::serial::graph_to_script(&self.graph);
            let sequence = self.last_sequence;
            storage.checkpoint(sequence, &script)?;
        }
        Ok(())
    }

    /// Parse and execute one or more `;`-separated statements atomically.
    ///
    /// Either every statement commits, or the graph is left unchanged.
    pub fn execute(&mut self, source: &str) -> Result<Vec<QueryResult>> {
        let mut transaction = self.begin();
        let results = transaction.execute(source)?;
        transaction.commit()?;
        Ok(results)
    }

    fn commit_graph(&mut self, graph: Graph, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }

        let sequence = self.last_sequence + 1;
        if let Some(storage) = &mut self.storage {
            let payload = sources.join("\n;\n");
            storage.append(sequence, payload.as_bytes())?;
        }

        self.graph = graph;
        self.last_sequence = sequence;
        Ok(())
    }

    fn execute_in_place(&mut self, source: &str) -> Result<Vec<QueryResult>> {
        let statements = parse_program(source)?;
        if statements.is_empty() {
            return Err(Error::new("empty query"));
        }

        let mut results = Vec::with_capacity(statements.len());
        for statement in &statements {
            results.push(self.execute_statement(statement)?);
        }
        Ok(results)
    }

    fn execute_statement(&mut self, statement: &Statement) -> Result<QueryResult> {
        match statement {
            Statement::Create { patterns, returns } => {
                self.execute_create(patterns, returns.as_ref())
            }
            Statement::Match {
                patterns,
                where_clause,
                action,
            } => self.execute_match(patterns, where_clause.as_ref(), action),
        }
    }

    fn execute_create(
        &mut self,
        patterns: &[Pattern],
        returns: Option<&Return>,
    ) -> Result<QueryResult> {
        let mut rows = vec![Row::new()];
        let mut stats = Stats::default();

        for pattern in patterns {
            let mut next_rows = Vec::new();
            for row in rows {
                next_rows.push(self.create_pattern(pattern, row, &mut stats)?);
            }
            rows = next_rows;
        }

        match returns {
            Some(ret) => {
                let mut result = self.project(&rows, ret)?;
                result.stats = stats;
                Ok(result)
            }
            None => Ok(QueryResult {
                stats,
                ..QueryResult::default()
            }),
        }
    }

    fn create_pattern(
        &mut self,
        pattern: &Pattern,
        mut row: Row,
        stats: &mut Stats,
    ) -> Result<Row> {
        let mut current = self.create_node(&pattern.start, &mut row, stats)?;

        for chain in &pattern.chains {
            let rel_type = chain.rel.rel_type.as_ref().ok_or_else(|| {
                Error::new("relationships in CREATE must have a type, for example `-[:KNOWS]->`")
            })?;

            let next = self.create_node(&chain.node, &mut row, stats)?;
            let props = self.eval_props(&chain.rel.props, &row)?;
            let edge_id = self
                .graph
                .add_edge(current, next, rel_type.clone(), props)?;
            stats.edges_created += 1;

            if let Some(variable) = &chain.rel.variable {
                if row.contains_key(variable) {
                    return Err(Error::new(format!(
                        "relationship variable `{variable}` is already bound"
                    )));
                }
                let edge = self.graph.edge(edge_id).expect("just created").clone();
                row.insert(variable.clone(), Binding::Edge(edge));
            }

            current = next;
        }

        Ok(row)
    }

    fn create_node(
        &mut self,
        pattern: &NodePattern,
        row: &mut Row,
        stats: &mut Stats,
    ) -> Result<NodeId> {
        if let Some(variable) = &pattern.variable {
            if let Some(binding) = row.get(variable) {
                return match binding {
                    Binding::Node(node) => {
                        if !pattern.labels.is_empty() || !pattern.props.is_empty() {
                            Err(Error::new(format!(
                                "node variable `{variable}` is already bound; in CREATE, refer to an existing node as `({variable})` without labels or properties"
                            )))
                        } else {
                            Ok(node.id)
                        }
                    }
                    _ => Err(Error::new(format!(
                        "`{variable}` is already bound to a {} and cannot be used as a node",
                        binding.as_value().type_name()
                    ))),
                };
            }
        }

        let props = self.eval_props(&pattern.props, row)?;
        let id = self.graph.add_node(pattern.labels.clone(), props);
        stats.nodes_created += 1;

        if let Some(variable) = &pattern.variable {
            let node = self.graph.node(id).expect("just created").clone();
            row.insert(variable.clone(), Binding::Node(node));
        }
        Ok(id)
    }

    fn execute_match(
        &mut self,
        patterns: &[Pattern],
        where_clause: Option<&Expr>,
        action: &MatchAction,
    ) -> Result<QueryResult> {
        let mut rows = self.match_patterns(patterns)?;

        if let Some(condition) = where_clause {
            let mut filtered = Vec::with_capacity(rows.len());
            for row in rows {
                if self.eval_expr(condition, &row)?.is_truthy() {
                    filtered.push(row);
                }
            }
            rows = filtered;
        }

        match action {
            MatchAction::Return(ret) => self.project(&rows, ret),
            MatchAction::Set {
                assignments,
                returns,
            } => {
                let (updated_rows, stats) = self.apply_set(&rows, assignments)?;
                let mut result = match returns {
                    Some(ret) => self.project(&updated_rows, ret)?,
                    None => QueryResult::default(),
                };
                result.stats.merge(&stats);
                Ok(result)
            }
            MatchAction::Delete {
                detach,
                targets,
                returns,
            } => {
                let stats = self.apply_delete(&rows, targets, *detach)?;
                let mut result = match returns {
                    Some(ret) => self.project(&rows, ret)?,
                    None => QueryResult::default(),
                };
                result.stats.merge(&stats);
                Ok(result)
            }
        }
    }

    // ---------------------------------------------------------------------
    // MATCH
    // ---------------------------------------------------------------------

    fn match_patterns(&self, patterns: &[Pattern]) -> Result<Vec<Row>> {
        let mut rows = vec![Row::new()];
        for pattern in patterns {
            let mut next = Vec::new();
            for row in rows {
                self.match_pattern(pattern, row, &mut next)?;
            }
            rows = next;
        }
        Ok(rows)
    }

    fn match_pattern(&self, pattern: &Pattern, row: Row, out: &mut Vec<Row>) -> Result<()> {
        for (node_id, extended) in self.candidate_nodes(&pattern.start, &row)? {
            self.extend_chains(&pattern.chains, 0, node_id, extended, out)?;
        }
        Ok(())
    }

    fn extend_chains(
        &self,
        chains: &[RelChain],
        index: usize,
        current: NodeId,
        row: Row,
        out: &mut Vec<Row>,
    ) -> Result<()> {
        if index == chains.len() {
            out.push(row);
            return Ok(());
        }

        let chain = &chains[index];
        for edge_id in self.edge_candidates(current, &chain.rel, &row)? {
            let edge = self
                .graph
                .edge(edge_id)
                .expect("edge candidate exists")
                .clone();

            let mut extended = row.clone();
            if let Some(variable) = &chain.rel.variable {
                if let Some(existing) = extended.get(variable) {
                    match existing {
                        Binding::Edge(bound) if bound.id == edge.id => {}
                        Binding::Edge(bound) => {
                            return Err(Error::new(format!(
                                "relationship variable `{variable}` is already bound to relationship #{}",
                                bound.id
                            )))
                        }
                        _ => {
                            return Err(Error::new(format!(
                                "`{variable}` is already bound to a non-relationship value"
                            )))
                        }
                    }
                } else {
                    extended.insert(variable.clone(), Binding::Edge(edge.clone()));
                }
            }

            let neighbor = match chain.rel.direction {
                Direction::LeftToRight => edge.to,
                Direction::RightToLeft => edge.from,
                Direction::Undirected => {
                    if edge.from == current {
                        edge.to
                    } else {
                        edge.from
                    }
                }
            };

            if let Some(next_row) = self.match_node(&chain.node, neighbor, &extended)? {
                self.extend_chains(chains, index + 1, neighbor, next_row, out)?;
            }
        }

        Ok(())
    }

    fn candidate_nodes(&self, pattern: &NodePattern, row: &Row) -> Result<Vec<(NodeId, Row)>> {
        if let Some(variable) = &pattern.variable {
            if let Some(binding) = row.get(variable) {
                return match binding {
                    Binding::Node(node) => match self.match_node(pattern, node.id, row)? {
                        Some(extended) => Ok(vec![(node.id, extended)]),
                        None => Ok(Vec::new()),
                    },
                    _ => Err(Error::new(format!(
                        "`{variable}` is already bound to a {} and cannot be used as a node",
                        binding.as_value().type_name()
                    ))),
                };
            }
        }

        let ids = self.candidate_ids(pattern, row)?;
        let mut matches = Vec::new();
        for id in ids {
            if let Some(extended) = self.match_node(pattern, id, row)? {
                matches.push((id, extended));
            }
        }
        Ok(matches)
    }

    fn candidate_ids(&self, pattern: &NodePattern, row: &Row) -> Result<Vec<NodeId>> {
        let mut ids = if let Some(label) = pattern.labels.first() {
            self.graph.nodes_with_label(label)
        } else {
            self.graph.node_ids()
        };

        if pattern.labels.len() > 1 {
            ids.retain(|id| {
                let node = self.graph.node(*id).expect("indexed node exists");
                pattern.labels.iter().all(|label| node.has_label(label))
            });
        }

        if !pattern.props.is_empty() {
            let mut filtered = Vec::new();
            for id in ids {
                if self.node_matches_props(id, &pattern.props, row)? {
                    filtered.push(id);
                }
            }
            ids = filtered;
        }

        Ok(ids)
    }

    fn match_node(&self, pattern: &NodePattern, id: NodeId, row: &Row) -> Result<Option<Row>> {
        let node = match self.graph.node(id) {
            Some(node) => node,
            None => return Ok(None),
        };

        if !pattern.labels.iter().all(|label| node.has_label(label)) {
            return Ok(None);
        }
        if !self.node_matches_props(id, &pattern.props, row)? {
            return Ok(None);
        }

        let mut extended = row.clone();
        if let Some(variable) = &pattern.variable {
            if let Some(binding) = extended.get(variable) {
                match binding {
                    Binding::Node(bound) if bound.id == id => {}
                    Binding::Node(bound) => {
                        return Err(Error::new(format!(
                            "node variable `{variable}` is already bound to node #{}",
                            bound.id
                        )))
                    }
                    _ => {
                        return Err(Error::new(format!(
                            "`{variable}` is already bound to a non-node value"
                        )))
                    }
                }
            } else {
                extended.insert(variable.clone(), Binding::Node(node.clone()));
            }
        }

        Ok(Some(extended))
    }

    fn node_matches_props(&self, id: NodeId, props: &[(String, Expr)], row: &Row) -> Result<bool> {
        if props.is_empty() {
            return Ok(true);
        }
        let node = match self.graph.node(id) {
            Some(node) => node,
            None => return Ok(false),
        };
        for (key, expr) in props {
            let expected = self.eval_expr(expr, row)?;
            match node.props.get(key) {
                Some(actual) if actual.strict_eq(&expected) => {}
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn edge_candidates(&self, node: NodeId, rel: &RelPattern, row: &Row) -> Result<Vec<EdgeId>> {
        let ids = match rel.direction {
            Direction::LeftToRight => self.graph.outgoing_ids(node),
            Direction::RightToLeft => self.graph.incoming_ids(node),
            Direction::Undirected => self.graph.incident_ids(node),
        };

        let mut matches = Vec::new();
        for id in ids {
            let edge = match self.graph.edge(id) {
                Some(edge) => edge,
                None => continue,
            };
            if let Some(rel_type) = &rel.rel_type {
                if &edge.rel_type != rel_type {
                    continue;
                }
            }

            let mut props_match = true;
            for (key, expr) in &rel.props {
                let expected = self.eval_expr(expr, row)?;
                match edge.props.get(key) {
                    Some(actual) if actual.strict_eq(&expected) => {}
                    _ => {
                        props_match = false;
                        break;
                    }
                }
            }
            if props_match {
                matches.push(id);
            }
        }
        Ok(matches)
    }

    // ---------------------------------------------------------------------
    // SET and DELETE
    // ---------------------------------------------------------------------

    fn apply_set(&mut self, rows: &[Row], assignments: &[Assignment]) -> Result<(Vec<Row>, Stats)> {
        let mut stats = Stats::default();
        let mut updated_rows = Vec::with_capacity(rows.len());

        for original in rows {
            let mut row = original.clone();
            for assignment in assignments {
                let (variable, key) = match &assignment.target {
                    Expr::Property(base, key) => match base.as_ref() {
                        Expr::Variable(variable) => (variable.clone(), key.clone()),
                        _ => {
                            return Err(Error::new(
                                "SET target must be a node or relationship property, for example `n.name`",
                            ))
                        }
                    },
                    _ => {
                        return Err(Error::new(
                            "SET target must be a property, for example `n.name`",
                        ))
                    }
                };

                let value = self.eval_expr(&assignment.value, &row)?;

                match row.get(&variable).cloned() {
                    Some(Binding::Node(node)) => {
                        self.graph.set_node_property(node.id, key.clone(), value)?;
                        let refreshed = self.graph.node(node.id).expect("node exists").clone();
                        row.insert(variable, Binding::Node(refreshed));
                    }
                    Some(Binding::Edge(edge)) => {
                        self.graph.set_edge_property(edge.id, key.clone(), value)?;
                        let refreshed = self.graph.edge(edge.id).expect("edge exists").clone();
                        row.insert(variable, Binding::Edge(refreshed));
                    }
                    None => {
                        return Err(Error::new(format!(
                            "cannot SET property on `{variable}`: variable is not bound"
                        )))
                    }
                }

                stats.properties_set += 1;
            }
            updated_rows.push(row);
        }

        Ok((updated_rows, stats))
    }

    fn apply_delete(&mut self, rows: &[Row], targets: &[Expr], detach: bool) -> Result<Stats> {
        let mut node_targets = Vec::new();
        let mut edge_targets = Vec::new();

        for row in rows {
            for target in targets {
                match self.eval_expr(target, row)? {
                    Value::Node(node) => node_targets.push(node.id),
                    Value::Edge(edge) => edge_targets.push(edge.id),
                    Value::Null => {}
                    other => {
                        return Err(Error::new(format!(
                            "DELETE expects nodes or relationships, found {}",
                            other.type_name()
                        )))
                    }
                }
            }
        }

        node_targets.sort_unstable();
        node_targets.dedup();
        edge_targets.sort_unstable();
        edge_targets.dedup();

        let mut stats = Stats::default();
        for id in edge_targets {
            if self.graph.remove_edge(id).is_some() {
                stats.edges_deleted += 1;
            }
        }
        for id in node_targets {
            if self.graph.node(id).is_none() {
                continue;
            }
            let removed_edges = self.graph.remove_node(id, detach)?;
            stats.nodes_deleted += 1;
            stats.edges_deleted += removed_edges;
        }

        Ok(stats)
    }

    // ---------------------------------------------------------------------
    // RETURN
    // ---------------------------------------------------------------------

    fn project(&self, rows: &[Row], ret: &Return) -> Result<QueryResult> {
        let items = self.expand_return_items(rows, ret);
        let aggregated = items.iter().any(|(_, expr)| contains_aggregate(expr));

        let mut projected: Vec<ProjectedRow> = Vec::new();

        if aggregated {
            for (_, expr) in &items {
                if contains_aggregate(expr) && aggregate_name(expr).is_none() {
                    return Err(Error::new(
                        "aggregate functions must be the entire RETURN item, for example `RETURN count(n)`",
                    ));
                }
            }

            let non_aggregate: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, (_, expr))| !contains_aggregate(expr))
                .map(|(index, _)| index)
                .collect();

            let mut group_order: Vec<Vec<Value>> = Vec::new();
            let mut groups: Vec<Vec<usize>> = Vec::new();
            let mut group_index: HashMap<String, usize> = HashMap::new();

            for (row_index, row) in rows.iter().enumerate() {
                let mut key_values = Vec::with_capacity(non_aggregate.len());
                for index in &non_aggregate {
                    key_values.push(self.eval_expr(&items[*index].1, row)?);
                }
                let key = key_values
                    .iter()
                    .map(Value::key)
                    .collect::<Vec<_>>()
                    .join("\u{1f}");

                match group_index.get(&key) {
                    Some(existing) => groups[*existing].push(row_index),
                    None => {
                        group_index.insert(key, group_order.len());
                        group_order.push(key_values);
                        groups.push(vec![row_index]);
                    }
                }
            }

            if groups.is_empty() && non_aggregate.is_empty() {
                group_order.push(Vec::new());
                groups.push(Vec::new());
            }

            for (group_number, member_rows) in groups.iter().enumerate() {
                let mut values = Vec::with_capacity(items.len());
                let mut key_iter = group_order[group_number].iter();
                for (_, expr) in &items {
                    if contains_aggregate(expr) {
                        let group_rows: Vec<&Row> =
                            member_rows.iter().map(|index| &rows[*index]).collect();
                        values.push(self.eval_aggregate(expr, &group_rows)?);
                    } else {
                        values.push(
                            key_iter
                                .next()
                                .cloned()
                                .expect("one key value per non-aggregate item"),
                        );
                    }
                }
                projected.push(ProjectedRow {
                    values,
                    source: member_rows.first().copied(),
                    group: Some(member_rows.clone()),
                });
            }
        } else {
            for (row_index, row) in rows.iter().enumerate() {
                let mut values = Vec::with_capacity(items.len());
                for (_, expr) in &items {
                    values.push(self.eval_expr(expr, row)?);
                }
                projected.push(ProjectedRow {
                    values,
                    source: Some(row_index),
                    group: None,
                });
            }
        }

        // ORDER BY is evaluated against the projection first (aliases), then
        // against the source row (or the aggregate group).
        let mut decorated = Vec::with_capacity(projected.len());
        for projected_row in projected {
            let mut keys = Vec::with_capacity(ret.order_by.len());
            for item in &ret.order_by {
                if let Expr::Variable(name) = &item.expr {
                    if let Some(index) = items.iter().position(|(column, _)| column == name) {
                        keys.push(projected_row.values[index].clone());
                        continue;
                    }
                }

                if let Some(group) = &projected_row.group {
                    if contains_aggregate(&item.expr) {
                        let group_rows: Vec<&Row> =
                            group.iter().map(|index| &rows[*index]).collect();
                        keys.push(self.eval_aggregate(&item.expr, &group_rows)?);
                    } else if let Some(first) = group.first() {
                        keys.push(self.eval_expr(&item.expr, &rows[*first])?);
                    } else {
                        keys.push(Value::Null);
                    }
                    continue;
                }

                let row = rows
                    .get(
                        projected_row
                            .source
                            .expect("non-aggregated rows have a source"),
                    )
                    .expect("source row exists");
                keys.push(self.eval_expr(&item.expr, row)?);
            }
            decorated.push((projected_row, keys));
        }

        decorated.sort_by(|left, right| {
            for (index, item) in ret.order_by.iter().enumerate() {
                let ordering = compare_values(&left.1[index], &right.1[index]);
                let ordering = if item.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            Ordering::Equal
        });

        if ret.distinct {
            let mut seen = std::collections::HashSet::new();
            decorated.retain(|(row, _)| {
                let key = row
                    .values
                    .iter()
                    .map(Value::key)
                    .collect::<Vec<_>>()
                    .join("\u{1f}");
                seen.insert(key)
            });
        }

        let skip = self.eval_count(ret.skip.as_ref(), "SKIP")?.unwrap_or(0);
        let limit = self.eval_count(ret.limit.as_ref(), "LIMIT")?;

        let mut output_rows: Vec<Vec<Value>> = decorated
            .into_iter()
            .skip(skip)
            .map(|(row, _)| row.values)
            .collect();
        if let Some(limit) = limit {
            output_rows.truncate(limit);
        }

        Ok(QueryResult {
            columns: items.into_iter().map(|(column, _)| column).collect(),
            rows: output_rows,
            stats: Stats::default(),
        })
    }

    fn expand_return_items(&self, rows: &[Row], ret: &Return) -> Vec<(String, Expr)> {
        let mut items = Vec::new();

        for item in &ret.items {
            if matches!(item.expr, Expr::Star) {
                let mut variables: Vec<String> = Vec::new();
                for row in rows {
                    for name in row.keys() {
                        if !variables.contains(name) {
                            variables.push(name.clone());
                        }
                    }
                }
                variables.sort();
                for variable in variables {
                    items.push((variable.clone(), Expr::Variable(variable)));
                }
                continue;
            }

            let column = item
                .alias
                .clone()
                .unwrap_or_else(|| item.expr.column_name());
            items.push((column, item.expr.clone()));
        }

        items
    }

    fn eval_aggregate(&self, expr: &Expr, rows: &[&Row]) -> Result<Value> {
        let (name, args) = match expr {
            Expr::Function { name, args } => (name.as_str(), args),
            _ => return Err(Error::new("expected an aggregate function")),
        };

        if !matches!(name, "count" | "sum" | "avg" | "min" | "max") {
            return Err(Error::new(format!(
                "`{name}` is not a supported aggregate function"
            )));
        }
        if args.len() != 1 {
            return Err(Error::new(format!(
                "aggregate function `{name}` expects one argument"
            )));
        }

        if name == "count" && matches!(args[0], Expr::Star) {
            return Ok(Value::Int(rows.len() as i64));
        }

        let mut values = Vec::new();
        for row in rows {
            let value = self.eval_expr(&args[0], row)?;
            if !matches!(value, Value::Null) {
                values.push(value);
            }
        }

        match name {
            "count" => Ok(Value::Int(values.len() as i64)),
            "sum" => {
                let mut integer_total: i64 = 0;
                let mut float_total: f64 = 0.0;
                let mut has_float = false;
                for value in &values {
                    match value {
                        Value::Int(number) => {
                            integer_total = checked_int(integer_total.checked_add(*number), "sum")?;
                            float_total += *number as f64;
                        }
                        Value::Float(number) => {
                            has_float = true;
                            float_total += number;
                        }
                        other => {
                            return Err(Error::new(format!(
                                "sum() expects numbers, found {}",
                                other.type_name()
                            )))
                        }
                    }
                }
                if has_float {
                    Ok(Value::Float(float_total))
                } else {
                    Ok(Value::Int(integer_total))
                }
            }
            "avg" => {
                if values.is_empty() {
                    return Ok(Value::Null);
                }
                let mut total = 0.0;
                for value in &values {
                    match value.as_f64() {
                        Some(number) => total += number,
                        None => {
                            return Err(Error::new(format!(
                                "avg() expects numbers, found {}",
                                value.type_name()
                            )))
                        }
                    }
                }
                Ok(Value::Float(total / values.len() as f64))
            }
            "min" | "max" => {
                let mut best: Option<Value> = None;
                for value in values {
                    best = Some(match best {
                        None => value,
                        Some(current) => {
                            let ordering = compare_values(&value, &current);
                            let take = if name == "min" {
                                ordering == Ordering::Less
                            } else {
                                ordering == Ordering::Greater
                            };
                            if take {
                                value
                            } else {
                                current
                            }
                        }
                    });
                }
                Ok(best.unwrap_or(Value::Null))
            }
            _ => unreachable!(),
        }
    }

    fn eval_count(&self, expr: Option<&Expr>, name: &str) -> Result<Option<usize>> {
        match expr {
            None => Ok(None),
            Some(expr) => {
                let value = self.eval_expr(expr, &Row::new())?;
                match value {
                    Value::Int(number) if number >= 0 => Ok(Some(number as usize)),
                    Value::Int(number) => Err(Error::new(format!(
                        "{name} must be a non-negative integer, found {number}"
                    ))),
                    other => Err(Error::new(format!(
                        "{name} must be an integer, found {}",
                        other.type_name()
                    ))),
                }
            }
        }
    }

    fn eval_props(&self, props: &[(String, Expr)], row: &Row) -> Result<BTreeMap<String, Value>> {
        let mut result = BTreeMap::new();
        for (key, expr) in props {
            result.insert(key.clone(), self.eval_expr(expr, row)?);
        }
        Ok(result)
    }

    fn eval_expr(&self, expr: &Expr, row: &Row) -> Result<Value> {
        match expr {
            Expr::Literal(value) => Ok(value.clone()),
            Expr::Variable(name) => Ok(row.get(name).map(Binding::as_value).unwrap_or(Value::Null)),
            Expr::Property(base, key) => {
                let value = self.eval_expr(base, row)?;
                match value {
                    Value::Node(node) => Ok(node.props.get(key).cloned().unwrap_or(Value::Null)),
                    Value::Edge(edge) => Ok(edge.props.get(key).cloned().unwrap_or(Value::Null)),
                    Value::Map(map) => Ok(map.get(key).cloned().unwrap_or(Value::Null)),
                    Value::Null => Ok(Value::Null),
                    other => Err(Error::new(format!(
                        "cannot access property `{key}` on a {}",
                        other.type_name()
                    ))),
                }
            }
            Expr::Binary { left, op, right } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    let left_value = self.eval_expr(left, row)?;
                    let left_truth = truth(&left_value);

                    if *op == BinaryOp::And && left_truth == Truth::False {
                        return Ok(Value::Bool(false));
                    }
                    if *op == BinaryOp::Or && left_truth == Truth::True {
                        return Ok(Value::Bool(true));
                    }

                    let right_value = self.eval_expr(right, row)?;
                    let right_truth = truth(&right_value);

                    return Ok(match op {
                        BinaryOp::And => {
                            if left_truth == Truth::False || right_truth == Truth::False {
                                Value::Bool(false)
                            } else if left_truth == Truth::Null || right_truth == Truth::Null {
                                Value::Null
                            } else {
                                Value::Bool(true)
                            }
                        }
                        BinaryOp::Or => {
                            if left_truth == Truth::True || right_truth == Truth::True {
                                Value::Bool(true)
                            } else if left_truth == Truth::Null || right_truth == Truth::Null {
                                Value::Null
                            } else {
                                Value::Bool(false)
                            }
                        }
                        _ => unreachable!(),
                    });
                }

                let left = self.eval_expr(left, row)?;
                let right = self.eval_expr(right, row)?;
                self.eval_binary(left, *op, right)
            }
            Expr::Unary { op, expr } => {
                let value = self.eval_expr(expr, row)?;
                match op {
                    UnaryOp::Not => Ok(match truth(&value) {
                        Truth::True => Value::Bool(false),
                        Truth::False => Value::Bool(true),
                        Truth::Null => Value::Null,
                    }),
                    UnaryOp::Negate => match value {
                        Value::Int(number) => {
                            Ok(Value::Int(checked_int(number.checked_neg(), "negate")?))
                        }
                        Value::Float(number) => Ok(Value::Float(-number)),
                        other => Err(Error::new(format!("cannot negate a {}", other.type_name()))),
                    },
                }
            }
            Expr::IsNull { expr, negated } => {
                let value = self.eval_expr(expr, row)?;
                let is_null = matches!(value, Value::Null);
                Ok(Value::Bool(if *negated { !is_null } else { is_null }))
            }
            Expr::Function { name, args } => self.eval_scalar_function(name, args, row),
            Expr::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval_expr(item, row)?);
                }
                Ok(Value::List(values))
            }
            Expr::Map(entries) => {
                let mut map = BTreeMap::new();
                for (key, expr) in entries {
                    map.insert(key.clone(), self.eval_expr(expr, row)?);
                }
                Ok(Value::Map(map))
            }
            Expr::Star => Err(Error::new("`*` is only valid in RETURN or count(*)")),
        }
    }

    fn eval_binary(&self, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
        match op {
            BinaryOp::Add => match (&left, &right) {
                (Value::String(a), Value::String(b)) => Ok(Value::String(format!("{a}{b}"))),
                (Value::Int(a), Value::Int(b)) => {
                    Ok(Value::Int(checked_int(a.checked_add(*b), "add")?))
                }
                (Value::Int(a), Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
                (Value::Float(a), Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                _ => Err(Error::new(format!(
                    "cannot add {} and {}",
                    left.type_name(),
                    right.type_name()
                ))),
            },
            BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => {
                if matches!(left, Value::Null) || matches!(right, Value::Null) {
                    return Ok(Value::Null);
                }

                if let (Some(a), Some(b)) = (left.as_f64(), right.as_f64()) {
                    let both_int = matches!(left, Value::Int(_)) && matches!(right, Value::Int(_));
                    if both_int {
                        let a = a as i64;
                        let b = b as i64;
                        return match op {
                            BinaryOp::Subtract => {
                                Ok(Value::Int(checked_int(a.checked_sub(b), "subtract")?))
                            }
                            BinaryOp::Multiply => {
                                Ok(Value::Int(checked_int(a.checked_mul(b), "multiply")?))
                            }
                            BinaryOp::Divide => {
                                if b == 0 {
                                    Err(Error::new("division by zero"))
                                } else {
                                    Ok(Value::Int(checked_int(a.checked_div(b), "divide")?))
                                }
                            }
                            BinaryOp::Modulo => {
                                if b == 0 {
                                    Err(Error::new("modulo by zero"))
                                } else {
                                    Ok(Value::Int(checked_int(a.checked_rem(b), "modulo")?))
                                }
                            }
                            _ => unreachable!(),
                        };
                    }

                    let result = match op {
                        BinaryOp::Subtract => a - b,
                        BinaryOp::Multiply => a * b,
                        BinaryOp::Divide => {
                            if b == 0.0 {
                                return Err(Error::new("division by zero"));
                            }
                            a / b
                        }
                        BinaryOp::Modulo => {
                            if b == 0.0 {
                                return Err(Error::new("modulo by zero"));
                            }
                            a % b
                        }
                        _ => unreachable!(),
                    };
                    return Ok(Value::Float(result));
                }

                Err(Error::new(format!(
                    "cannot use {} with {} and {}",
                    op.symbol(),
                    left.type_name(),
                    right.type_name()
                )))
            }
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Lt
            | BinaryOp::LtEq
            | BinaryOp::Gt
            | BinaryOp::GtEq => {
                if matches!(left, Value::Null) || matches!(right, Value::Null) {
                    return Ok(Value::Null);
                }

                let result = match op {
                    BinaryOp::Eq => left.strict_eq(&right),
                    BinaryOp::NotEq => !left.strict_eq(&right),
                    BinaryOp::Lt => compare_values(&left, &right) == Ordering::Less,
                    BinaryOp::LtEq => {
                        matches!(
                            compare_values(&left, &right),
                            Ordering::Less | Ordering::Equal
                        )
                    }
                    BinaryOp::Gt => compare_values(&left, &right) == Ordering::Greater,
                    BinaryOp::GtEq => {
                        matches!(
                            compare_values(&left, &right),
                            Ordering::Greater | Ordering::Equal
                        )
                    }
                    _ => unreachable!(),
                };
                Ok(Value::Bool(result))
            }
            BinaryOp::And | BinaryOp::Or => unreachable!("handled by caller"),
        }
    }

    fn eval_scalar_function(&self, name: &str, args: &[Expr], row: &Row) -> Result<Value> {
        match name {
            "id" => {
                require_args(name, args, 1)?;
                let value = self.eval_expr(&args[0], row)?;
                match value {
                    Value::Node(node) => Ok(Value::Int(node.id as i64)),
                    Value::Edge(edge) => Ok(Value::Int(edge.id as i64)),
                    other => Err(Error::new(format!(
                        "id() expects a node or relationship, found {}",
                        other.type_name()
                    ))),
                }
            }
            "labels" => {
                require_args(name, args, 1)?;
                let value = self.eval_expr(&args[0], row)?;
                match value {
                    Value::Node(node) => Ok(Value::List(
                        node.labels
                            .iter()
                            .map(|label| Value::String(label.clone()))
                            .collect(),
                    )),
                    other => Err(Error::new(format!(
                        "labels() expects a node, found {}",
                        other.type_name()
                    ))),
                }
            }
            "type" => {
                require_args(name, args, 1)?;
                let value = self.eval_expr(&args[0], row)?;
                match value {
                    Value::Edge(edge) => Ok(Value::String(edge.rel_type.clone())),
                    other => Err(Error::new(format!(
                        "type() expects a relationship, found {}",
                        other.type_name()
                    ))),
                }
            }
            "keys" => {
                require_args(name, args, 1)?;
                let value = self.eval_expr(&args[0], row)?;
                let keys: Vec<String> = match value {
                    Value::Node(node) => node.props.keys().cloned().collect(),
                    Value::Edge(edge) => edge.props.keys().cloned().collect(),
                    Value::Map(map) => map.keys().cloned().collect(),
                    other => {
                        return Err(Error::new(format!(
                            "keys() expects a node, relationship, or map, found {}",
                            other.type_name()
                        )))
                    }
                };
                Ok(Value::List(keys.into_iter().map(Value::String).collect()))
            }
            "coalesce" => {
                if args.is_empty() {
                    return Err(Error::new("coalesce() expects at least one argument"));
                }
                for arg in args {
                    let value = self.eval_expr(arg, row)?;
                    if !matches!(value, Value::Null) {
                        return Ok(value);
                    }
                }
                Ok(Value::Null)
            }
            "tolower" | "lower" => {
                require_args(name, args, 1)?;
                let value = self.eval_expr(&args[0], row)?;
                match value {
                    Value::String(value) => Ok(Value::String(value.to_lowercase())),
                    Value::Null => Ok(Value::Null),
                    other => Err(Error::new(format!(
                        "{name}() expects a string, found {}",
                        other.type_name()
                    ))),
                }
            }
            "toupper" | "upper" => {
                require_args(name, args, 1)?;
                let value = self.eval_expr(&args[0], row)?;
                match value {
                    Value::String(value) => Ok(Value::String(value.to_uppercase())),
                    Value::Null => Ok(Value::Null),
                    other => Err(Error::new(format!(
                        "{name}() expects a string, found {}",
                        other.type_name()
                    ))),
                }
            }
            "count" | "sum" | "avg" | "min" | "max" => Err(Error::new(format!(
                "aggregate function `{name}` can only be used as a RETURN item"
            ))),
            other => Err(Error::new(format!("unknown function `{other}`"))),
        }
    }
}

/// An explicit multi-statement transaction.
///
/// A transaction stages its first write in a graph copy. `commit` appends one
/// durable WAL record and installs the staged graph; dropping or calling
/// [`Transaction::rollback`] discards it.
pub struct Transaction<'a> {
    db: &'a mut Database,
    staged: Option<Graph>,
    sources: Vec<String>,
    failed: bool,
}

impl<'a> Transaction<'a> {
    /// Execute statements inside this transaction.
    pub fn execute(&mut self, source: &str) -> Result<Vec<QueryResult>> {
        if self.failed {
            return Err(Error::new(
                "transaction is aborted; start a new transaction to continue",
            ));
        }

        let statements = match parse_program(source) {
            Ok(statements) => statements,
            Err(error) => {
                self.failed = true;
                return Err(error);
            }
        };
        if statements.is_empty() {
            self.failed = true;
            return Err(Error::new("empty query"));
        }

        let writes = statements.iter().any(Statement::is_write);

        if !writes && self.staged.is_none() {
            let result = self.db.execute_in_place(source);
            if result.is_err() {
                self.failed = true;
            }
            return result;
        }

        if self.staged.is_none() {
            self.staged = Some(self.db.graph.clone());
        }

        let live = std::mem::replace(
            &mut self.db.graph,
            self.staged.take().expect("staged graph exists"),
        );
        let result = self.db.execute_in_place(source);
        self.staged = Some(std::mem::replace(&mut self.db.graph, live));

        match result {
            Ok(results) => {
                if writes {
                    self.sources.push(source.to_string());
                }
                Ok(results)
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    /// Durably commit the transaction.
    pub fn commit(mut self) -> Result<()> {
        if self.failed {
            return Err(Error::new(
                "cannot commit: the transaction is aborted; no changes were applied",
            ));
        }

        match self.staged.take() {
            Some(graph) => {
                let sources = std::mem::take(&mut self.sources);
                self.db.commit_graph(graph, &sources)
            }
            None => Ok(()),
        }
    }

    /// Discard all changes made inside the transaction.
    pub fn rollback(self) {}

    pub fn is_failed(&self) -> bool {
        self.failed
    }
}

fn require_args(name: &str, args: &[Expr], expected: usize) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(Error::new(format!(
            "{name}() expects {expected} argument(s), found {}",
            args.len()
        )))
    }
}

fn checked_int(result: Option<i64>, operation: &str) -> Result<i64> {
    result.ok_or_else(|| Error::new(format!("integer overflow while trying to {operation}")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Truth {
    True,
    False,
    Null,
}

/// Graphite's predicate truthiness: only `true` is true, `null` propagates,
/// and every non-boolean, non-null value behaves as false.
fn truth(value: &Value) -> Truth {
    match value {
        Value::Bool(true) => Truth::True,
        Value::Null => Truth::Null,
        _ => Truth::False,
    }
}

fn aggregate_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Function { name, .. }
            if matches!(name.as_str(), "count" | "sum" | "avg" | "min" | "max") =>
        {
            Some(name)
        }
        _ => None,
    }
}

fn contains_aggregate(expr: &Expr) -> bool {
    if aggregate_name(expr).is_some() {
        return true;
    }
    match expr {
        Expr::Property(base, _) => contains_aggregate(base),
        Expr::Binary { left, right, .. } => contains_aggregate(left) || contains_aggregate(right),
        Expr::Unary { expr, .. } | Expr::IsNull { expr, .. } => contains_aggregate(expr),
        Expr::Function { args, .. } => args.iter().any(contains_aggregate),
        Expr::List(items) => items.iter().any(contains_aggregate),
        Expr::Map(entries) => entries.iter().any(|(_, value)| contains_aggregate(value)),
        _ => false,
    }
}

fn compare_values(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        _ => match left.compare(right) {
            Some(ordering) => ordering,
            None => left.type_rank().cmp(&right.type_rank()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(db: &mut Database, query: &str) -> Vec<Vec<Value>> {
        db.execute(query).unwrap().into_iter().next().unwrap().rows
    }

    #[test]
    fn creates_and_reads_nodes() {
        let mut db = Database::new();
        db.execute(r#"CREATE (a:Person {name: "Alice", age: 30})"#)
            .unwrap();
        let result = rows(&mut db, "MATCH (n:Person) RETURN n.name, n.age");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], Value::String("Alice".into()));
        assert_eq!(result[0][1], Value::Int(30));
    }

    #[test]
    fn matches_relationships() {
        let mut db = Database::new();
        db.execute(r#"CREATE (a:Person {name: "Alice"})-[:KNOWS]->(b:Person {name: "Bob"})"#)
            .unwrap();

        let result = rows(
            &mut db,
            r#"MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name"#,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], Value::String("Alice".into()));
        assert_eq!(result[0][1], Value::String("Bob".into()));
    }

    #[test]
    fn filters_orders_and_limits() {
        let mut db = Database::new();
        db.execute(
            r#"CREATE (:Person {name: "A", age: 40}), (:Person {name: "B", age: 20}), (:Person {name: "C", age: 30})"#,
        )
        .unwrap();

        let result = rows(
            &mut db,
            "MATCH (n:Person) WHERE n.age >= 30 RETURN n.name ORDER BY n.age DESC LIMIT 1",
        );
        assert_eq!(result, vec![vec![Value::String("A".into())]]);
    }

    #[test]
    fn counts_and_groups() {
        let mut db = Database::new();
        db.execute(
            r#"CREATE (:Person {name: "A", city: "X"}), (:Person {name: "B", city: "X"}), (:Person {name: "C", city: "Y"})"#,
        )
        .unwrap();

        let result = rows(
            &mut db,
            "MATCH (n:Person) RETURN n.city AS city, count(*) AS people ORDER BY city",
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0][1], Value::Int(2));
        assert_eq!(result[1][1], Value::Int(1));
    }
}
