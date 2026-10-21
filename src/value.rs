use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use crate::graph::{Edge, Node};

/// A scalar or structural value stored in the graph or produced by a query.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    Node(Box<Node>),
    Edge(Box<Edge>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Node(_) => "node",
            Value::Edge(_) => "relationship",
        }
    }

    /// Truthiness used by WHERE/AND/OR/NOT.
    ///
    /// Only `true` is true. Everything else (including null) is false.
    pub fn is_truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Equality used by MATCH property filters and `=`. Nulls are equal to
    /// nulls here (unlike SQL's three-valued comparison) because graph
    /// property matching is more useful that way.
    pub fn strict_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
                (*a as f64) == *b
            }
            (Value::String(a), Value::String(b)) => a == b,
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.strict_eq(y))
            }
            (Value::Map(a), Value::Map(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.get(key).is_some_and(|v| v.strict_eq(value)))
            }
            (Value::Node(a), Value::Node(b)) => a.id == b.id,
            (Value::Edge(a), Value::Edge(b)) => a.id == b.id,
            _ => false,
        }
    }

    /// Total-ish ordering used by ORDER BY, min(), and max().
    ///
    /// Values of different types sort by a stable type rank. Nulls are
    /// handled by the caller so that they can be placed deterministically.
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        use Value::*;
        match (self, other) {
            (Null, Null) => Some(Ordering::Equal),
            (Bool(a), Bool(b)) => Some(a.cmp(b)),
            (Int(a), Int(b)) => Some(a.cmp(b)),
            (Float(a), Float(b)) => a.partial_cmp(b),
            (Int(a), Float(b)) => (*a as f64).partial_cmp(b),
            (Float(a), Int(b)) => a.partial_cmp(&(*b as f64)),
            (String(a), String(b)) => Some(a.cmp(b)),
            (Node(a), Node(b)) => Some(a.id.cmp(&b.id)),
            (Edge(a), Edge(b)) => Some(a.id.cmp(&b.id)),
            _ => None,
        }
    }

    pub fn type_rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) | Value::Float(_) => 2,
            Value::String(_) => 3,
            Value::List(_) => 4,
            Value::Map(_) => 5,
            Value::Node(_) => 6,
            Value::Edge(_) => 7,
        }
    }

    /// A stable string key used for grouping and DISTINCT.
    pub fn key(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(b) => format!("bool:{b}"),
            Value::Int(i) => format!("num:{i}"),
            Value::Float(f) => format!("num:{f}"),
            Value::String(s) => format!("str:{s}"),
            Value::List(values) => {
                let inner: Vec<String> = values.iter().map(Value::key).collect();
                format!("list:[{}]", inner.join(","))
            }
            Value::Map(map) => {
                let inner: Vec<String> = map
                    .iter()
                    .map(|(key, value)| format!("{key}={}", value.key()))
                    .collect();
                format!("map:{{{}}}", inner.join(","))
            }
            Value::Node(node) => format!("node:{}", node.id),
            Value::Edge(edge) => format!("edge:{}", edge.id),
        }
    }

    /// Render a value the way it would be written in a query.
    pub fn literal(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(true) => "true".to_string(),
            Value::Bool(false) => "false".to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => {
                if f.fract() == 0.0 {
                    format!("{f:.1}")
                } else {
                    f.to_string()
                }
            }
            Value::String(s) => quote(s),
            Value::List(values) => {
                let inner: Vec<String> = values.iter().map(Value::literal).collect();
                format!("[{}]", inner.join(", "))
            }
            Value::Map(map) => {
                let inner: Vec<String> = map
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key_literal(key), value.literal()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            Value::Node(node) => node_signature(node),
            Value::Edge(edge) => format!("[:{}{}]", edge.rel_type, props_suffix(&edge.props)),
        }
    }
}

pub fn quote(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub fn props_suffix(props: &BTreeMap<String, Value>) -> String {
    if props.is_empty() {
        return String::new();
    }
    let inner: Vec<String> = props
        .iter()
        .map(|(key, value)| format!("{}: {}", key_literal(key), value.literal()))
        .collect();
    format!(" {{{}}}", inner.join(", "))
}

/// Render a property key so that it can be parsed back as part of a map.
/// Identifiers are emitted bare; anything else is quoted.
pub fn key_literal(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_alphabetic() || first == '_' => {
            if chars.all(|ch| ch.is_alphanumeric() || ch == '_') {
                key.to_string()
            } else {
                quote(key)
            }
        }
        _ => quote(key),
    }
}

pub fn node_signature(node: &Node) -> String {
    let labels = if node.labels.is_empty() {
        String::new()
    } else {
        node.labels
            .iter()
            .map(|label| format!(":{label}"))
            .collect::<Vec<_>>()
            .join("")
    };
    format!("({}{})", labels, props_suffix(&node.props))
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.literal())
    }
}
