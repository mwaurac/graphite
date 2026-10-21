use crate::value::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Match {
        patterns: Vec<Pattern>,
        where_clause: Option<Expr>,
        action: MatchAction,
    },
    Create {
        patterns: Vec<Pattern>,
        returns: Option<Return>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MatchAction {
    Return(Return),
    Set {
        assignments: Vec<Assignment>,
        returns: Option<Return>,
    },
    Delete {
        detach: bool,
        targets: Vec<Expr>,
        returns: Option<Return>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Assignment {
    pub target: Expr,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Return {
    pub distinct: bool,
    pub items: Vec<ReturnItem>,
    pub order_by: Vec<OrderItem>,
    pub skip: Option<Expr>,
    pub limit: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReturnItem {
    pub expr: Expr,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderItem {
    pub expr: Expr,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub start: NodePattern,
    pub chains: Vec<RelChain>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelChain {
    pub rel: RelPattern,
    pub node: NodePattern,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub props: Vec<(String, Expr)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelPattern {
    pub variable: Option<String>,
    pub rel_type: Option<String>,
    pub props: Vec<(String, Expr)>,
    pub direction: Direction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
    Undirected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Subtract => "-",
            BinaryOp::Multiply => "*",
            BinaryOp::Divide => "/",
            BinaryOp::Modulo => "%",
            BinaryOp::Eq => "=",
            BinaryOp::NotEq => "<>",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Negate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal(Value),
    Variable(String),
    Property(Box<Expr>, String),
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Function {
        name: String,
        args: Vec<Expr>,
    },
    List(Vec<Expr>),
    Map(Vec<(String, Expr)>),
    /// Only valid as a RETURN item or inside `count(*)`.
    Star,
}

impl Expr {
    pub fn column_name(&self) -> String {
        match self {
            Expr::Variable(name) => name.clone(),
            Expr::Property(base, key) => format!("{}.{}", base.column_name(), key),
            Expr::Function { name, args } => {
                let args: Vec<String> = args.iter().map(Expr::column_name).collect();
                format!("{}({})", name.to_lowercase(), args.join(", "))
            }
            Expr::Star => "*".to_string(),
            other => other.to_string(),
        }
    }
}

impl std::fmt::Display for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::Literal(value) => f.write_str(&value.literal()),
            Expr::Variable(name) => f.write_str(name),
            Expr::Property(base, key) => write!(f, "{base}.{key}"),
            Expr::Binary { left, op, right } => write!(f, "({left} {} {right})", op.symbol()),
            Expr::Unary { op, expr } => match op {
                UnaryOp::Not => write!(f, "NOT ({expr})"),
                UnaryOp::Negate => write!(f, "-({expr})"),
            },
            Expr::IsNull { expr, negated } => {
                if *negated {
                    write!(f, "({expr} IS NOT NULL)")
                } else {
                    write!(f, "({expr} IS NULL)")
                }
            }
            Expr::Function { name, args } => {
                let args: Vec<String> = args.iter().map(ToString::to_string).collect();
                write!(f, "{}({})", name.to_lowercase(), args.join(", "))
            }
            Expr::List(items) => {
                let items: Vec<String> = items.iter().map(ToString::to_string).collect();
                write!(f, "[{}]", items.join(", "))
            }
            Expr::Map(entries) => {
                let entries: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}"))
                    .collect();
                write!(f, "{{{}}}", entries.join(", "))
            }
            Expr::Star => f.write_str("*"),
        }
    }
}

impl NodePattern {
    pub fn describe(&self) -> String {
        let mut out = String::from("(");
        if let Some(variable) = &self.variable {
            out.push_str(variable);
        }
        for label in &self.labels {
            out.push(':');
            out.push_str(label);
        }
        if !self.props.is_empty() {
            let props: Vec<String> = self
                .props
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect();
            out.push_str(&format!(" {{{}}}", props.join(", ")));
        }
        out.push(')');
        out
    }
}

impl RelPattern {
    pub fn describe(&self, node_description: &str) -> String {
        let mut inner = String::new();
        if let Some(variable) = &self.variable {
            inner.push_str(variable);
        }
        if let Some(rel_type) = &self.rel_type {
            inner.push(':');
            inner.push_str(rel_type);
        }
        if !self.props.is_empty() {
            let props: Vec<String> = self
                .props
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect();
            inner.push_str(&format!(" {{{}}}", props.join(", ")));
        }

        match self.direction {
            Direction::LeftToRight => format!("-[{inner}]->{node_description}"),
            Direction::RightToLeft => format!("<-[{inner}]-{node_description}"),
            Direction::Undirected => format!("-[{inner}]-{node_description}"),
        }
    }
}
