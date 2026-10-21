use crate::ast::{
    Assignment, BinaryOp, Direction, Expr, MatchAction, NodePattern, OrderItem, Pattern, RelChain,
    RelPattern, Return, ReturnItem, Statement, UnaryOp,
};
use crate::error::{Error, Result};
use crate::lexer::{lex, Token, TokenKind};
use crate::value::Value;

pub fn parse_program(source: &str) -> Result<Vec<Statement>> {
    let tokens = lex(source)?;
    Parser::new(tokens).parse_program()
}

pub fn parse_statement(source: &str) -> Result<Statement> {
    let mut statements = parse_program(source)?;
    match statements.len() {
        0 => Err(Error::new("empty query")),
        1 => Ok(statements.remove(0)),
        _ => Err(Error::new(
            "expected a single statement; use `;` separated statements only in a script",
        )),
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            position: 0,
        }
    }

    fn parse_program(mut self) -> Result<Vec<Statement>> {
        let mut statements = Vec::new();
        loop {
            while self.check(&TokenKind::Semicolon) {
                self.advance();
            }
            if self.check(&TokenKind::Eof) {
                break;
            }

            statements.push(self.parse_statement()?);

            if self.check(&TokenKind::Semicolon) {
                self.advance();
            } else if !self.check(&TokenKind::Eof) {
                let token = self.peek().clone();
                return Err(self.error_at(
                    &token,
                    format!(
                        "expected `;` or end of input, found {}",
                        token.kind.describe()
                    ),
                ));
            }
        }
        Ok(statements)
    }

    fn parse_statement(&mut self) -> Result<Statement> {
        if self.check_keyword("MATCH") {
            self.parse_match()
        } else if self.check_keyword("CREATE") {
            self.parse_create()
        } else {
            let token = self.peek().clone();
            Err(self.error_at(
                &token,
                format!("expected MATCH or CREATE, found {}", token.kind.describe()),
            ))
        }
    }

    fn parse_match(&mut self) -> Result<Statement> {
        self.expect_keyword("MATCH")?;
        let patterns = self.parse_patterns()?;

        let where_clause = if self.eat_keyword("WHERE") {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let action = if self.eat_keyword("RETURN") {
            MatchAction::Return(self.parse_return_after_keyword()?)
        } else if self.eat_keyword("SET") {
            let assignments = self.parse_assignments()?;
            let returns = if self.eat_keyword("RETURN") {
                Some(self.parse_return_after_keyword()?)
            } else {
                None
            };
            MatchAction::Set {
                assignments,
                returns,
            }
        } else {
            let detach = self.eat_keyword("DETACH");
            if self.eat_keyword("DELETE") {
                let targets = self.parse_expr_list()?;
                let returns = if self.eat_keyword("RETURN") {
                    Some(self.parse_return_after_keyword()?)
                } else {
                    None
                };
                MatchAction::Delete {
                    detach,
                    targets,
                    returns,
                }
            } else {
                let token = self.peek().clone();
                return Err(self.error_at(
                    &token,
                    "a MATCH clause must be followed by RETURN, SET, or DELETE",
                ));
            }
        };

        Ok(Statement::Match {
            patterns,
            where_clause,
            action,
        })
    }

    fn parse_create(&mut self) -> Result<Statement> {
        self.expect_keyword("CREATE")?;
        let patterns = self.parse_patterns()?;
        let returns = if self.eat_keyword("RETURN") {
            Some(self.parse_return_after_keyword()?)
        } else {
            None
        };
        Ok(Statement::Create { patterns, returns })
    }

    fn parse_patterns(&mut self) -> Result<Vec<Pattern>> {
        let mut patterns = vec![self.parse_pattern()?];
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_pattern()?);
        }
        Ok(patterns)
    }

    fn parse_pattern(&mut self) -> Result<Pattern> {
        let start = self.parse_node_pattern()?;
        let mut chains = Vec::new();
        while matches!(self.peek().kind, TokenKind::Minus | TokenKind::ArrowLeft) {
            let rel = self.parse_rel_pattern()?;
            let node = self.parse_node_pattern()?;
            chains.push(RelChain { rel, node });
        }
        Ok(Pattern { start, chains })
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern> {
        self.expect(&TokenKind::LParen)?;

        let variable = match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Some(name)
            }
            _ => None,
        };

        let mut labels = Vec::new();
        while self.eat(&TokenKind::Colon) {
            labels.push(self.expect_ident("a label name")?);
        }

        let props = if self.check(&TokenKind::LBrace) {
            self.parse_property_map()?
        } else {
            Vec::new()
        };

        self.expect(&TokenKind::RParen)?;
        Ok(NodePattern {
            variable,
            labels,
            props,
        })
    }

    fn parse_rel_pattern(&mut self) -> Result<RelPattern> {
        let first = self.advance();
        let left = match &first.kind {
            TokenKind::ArrowLeft => Some(Direction::RightToLeft),
            TokenKind::Minus => None,
            other => {
                return Err(self.error_at(
                    &first,
                    format!(
                        "expected a relationship pattern, found {}",
                        other.describe()
                    ),
                ))
            }
        };

        self.expect(&TokenKind::LBracket)?;

        let variable = match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Some(name)
            }
            _ => None,
        };

        let rel_type = if self.eat(&TokenKind::Colon) {
            Some(self.expect_ident("a relationship type")?)
        } else {
            None
        };

        let props = if self.check(&TokenKind::LBrace) {
            self.parse_property_map()?
        } else {
            Vec::new()
        };

        self.expect(&TokenKind::RBracket)?;

        let last = self.advance();
        let right_arrow = match &last.kind {
            TokenKind::ArrowRight => true,
            TokenKind::Minus => false,
            other => {
                return Err(self.error_at(
                    &last,
                    format!(
                        "expected `->` or `-` to close the relationship pattern, found {}",
                        other.describe()
                    ),
                ))
            }
        };

        let direction = match (left, right_arrow) {
            (Some(Direction::RightToLeft), false) => Direction::RightToLeft,
            (None, true) => Direction::LeftToRight,
            (None, false) => Direction::Undirected,
            _ => {
                return Err(self.error_at(
                    &last,
                    "relationship direction cannot point both ways; use `-[...]->`, `<-[...]-`, or `-[...]-`",
                ))
            }
        };

        Ok(RelPattern {
            variable,
            rel_type,
            props,
            direction,
        })
    }

    fn parse_property_map(&mut self) -> Result<Vec<(String, Expr)>> {
        self.expect(&TokenKind::LBrace)?;
        let mut entries = Vec::new();

        if self.eat(&TokenKind::RBrace) {
            return Ok(entries);
        }

        loop {
            let key = match &self.peek().kind {
                TokenKind::Ident(name) => {
                    let name = name.clone();
                    self.advance();
                    name
                }
                TokenKind::String(value) => {
                    let value = value.clone();
                    self.advance();
                    value
                }
                _ => {
                    let token = self.peek().clone();
                    return Err(self.error_at(&token, "expected a property name"));
                }
            };
            self.expect(&TokenKind::Colon)?;
            let value = self.parse_expr()?;
            entries.push((key, value));

            if self.eat(&TokenKind::Comma) {
                continue;
            }
            if self.eat(&TokenKind::RBrace) {
                break;
            }
            let token = self.peek().clone();
            return Err(self.error_at(
                &token,
                format!("expected `,` or `}}`, found {}", token.kind.describe()),
            ));
        }

        Ok(entries)
    }

    fn parse_return_after_keyword(&mut self) -> Result<Return> {
        let distinct = self.eat_keyword("DISTINCT");
        let mut items = Vec::new();

        loop {
            if self.eat(&TokenKind::Star) {
                items.push(ReturnItem {
                    expr: Expr::Star,
                    alias: None,
                });
            } else {
                let expr = self.parse_expr()?;
                let alias = if self.eat_keyword("AS") {
                    Some(self.expect_ident("an alias")?)
                } else {
                    None
                };
                items.push(ReturnItem { expr, alias });
            }

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let mut order_by = Vec::new();
        if self.eat_keyword("ORDER") {
            self.expect_keyword("BY")?;
            loop {
                let expr = self.parse_expr()?;
                let descending = if self.eat_keyword("DESC") {
                    true
                } else {
                    self.eat_keyword("ASC");
                    false
                };
                order_by.push(OrderItem { expr, descending });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }

        let mut skip = None;
        let mut limit = None;
        loop {
            if self.eat_keyword("SKIP") {
                if skip.is_some() {
                    return Err(Error::new("SKIP can only be specified once"));
                }
                skip = Some(self.parse_expr()?);
            } else if self.eat_keyword("LIMIT") {
                if limit.is_some() {
                    return Err(Error::new("LIMIT can only be specified once"));
                }
                limit = Some(self.parse_expr()?);
            } else {
                break;
            }
        }

        Ok(Return {
            distinct,
            items,
            order_by,
            skip,
            limit,
        })
    }

    fn parse_assignments(&mut self) -> Result<Vec<Assignment>> {
        let mut assignments = Vec::new();
        loop {
            let target = self.parse_assignment_target()?;
            self.expect(&TokenKind::Eq)?;
            let value = self.parse_expr()?;
            assignments.push(Assignment { target, value });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(assignments)
    }

    /// SET targets are restricted to `variable.property` so that `=` is
    /// unambiguously the assignment operator rather than a comparison.
    fn parse_assignment_target(&mut self) -> Result<Expr> {
        let variable = self.expect_ident("a variable to assign")?;
        let mut expr = Expr::Variable(variable);
        while self.eat(&TokenKind::Dot) {
            let key = self.expect_ident("a property name")?;
            expr = Expr::Property(Box::new(expr), key);
        }
        Ok(expr)
    }

    fn parse_expr_list(&mut self) -> Result<Vec<Expr>> {
        let mut expressions = vec![self.parse_expr()?];
        while self.eat(&TokenKind::Comma) {
            expressions.push(self.parse_expr()?);
        }
        Ok(expressions)
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.eat_keyword("OR") {
            let right = self.parse_and()?;
            left = Expr::Binary {
                left: Box::new(left),
                op: BinaryOp::Or,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_not()?;
        while self.eat_keyword("AND") {
            let right = self.parse_not()?;
            left = Expr::Binary {
                left: Box::new(left),
                op: BinaryOp::And,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr> {
        if self.eat_keyword("NOT") {
            let expr = self.parse_not()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            });
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<Expr> {
        let left = self.parse_additive()?;

        if self.eat_keyword("IS") {
            let negated = self.eat_keyword("NOT");
            self.expect_keyword("NULL")?;
            return Ok(Expr::IsNull {
                expr: Box::new(left),
                negated,
            });
        }

        let op = if self.eat(&TokenKind::Eq) {
            Some(BinaryOp::Eq)
        } else if self.eat(&TokenKind::NotEq) {
            Some(BinaryOp::NotEq)
        } else if self.eat(&TokenKind::LtEq) {
            Some(BinaryOp::LtEq)
        } else if self.eat(&TokenKind::Lt) {
            Some(BinaryOp::Lt)
        } else if self.eat(&TokenKind::GtEq) {
            Some(BinaryOp::GtEq)
        } else if self.eat(&TokenKind::Gt) {
            Some(BinaryOp::Gt)
        } else {
            None
        };

        match op {
            Some(op) => {
                let right = self.parse_additive()?;
                Ok(Expr::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                })
            }
            None => Ok(left),
        }
    }

    fn parse_additive(&mut self) -> Result<Expr> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = if self.eat(&TokenKind::Plus) {
                Some(BinaryOp::Add)
            } else if self.eat(&TokenKind::Minus) {
                Some(BinaryOp::Subtract)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_multiplicative()?;
                    left = Expr::Binary {
                        left: Box::new(left),
                        op,
                        right: Box::new(right),
                    };
                }
                None => break,
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            let op = if self.eat(&TokenKind::Star) {
                Some(BinaryOp::Multiply)
            } else if self.eat(&TokenKind::Slash) {
                Some(BinaryOp::Divide)
            } else if self.eat(&TokenKind::Percent) {
                Some(BinaryOp::Modulo)
            } else {
                None
            };
            match op {
                Some(op) => {
                    let right = self.parse_unary()?;
                    left = Expr::Binary {
                        left: Box::new(left),
                        op,
                        right: Box::new(right),
                    };
                }
                None => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.eat(&TokenKind::Minus) {
            let expr = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Negate,
                expr: Box::new(expr),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        if self.check(&TokenKind::LBrace) {
            return Ok(Expr::Map(self.parse_property_map()?));
        }

        let token = self.advance();
        let mut expr = match &token.kind {
            TokenKind::Int(value) => Expr::Literal(Value::Int(*value)),
            TokenKind::Float(value) => Expr::Literal(Value::Float(*value)),
            TokenKind::String(value) => Expr::Literal(Value::String(value.clone())),
            TokenKind::Star => Expr::Star,
            TokenKind::LParen => {
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                inner
            }
            TokenKind::LBracket => {
                let mut items = Vec::new();
                if !self.eat(&TokenKind::RBracket) {
                    loop {
                        items.push(self.parse_expr()?);
                        if self.eat(&TokenKind::Comma) {
                            continue;
                        }
                        self.expect(&TokenKind::RBracket)?;
                        break;
                    }
                }
                Expr::List(items)
            }
            TokenKind::Ident(name) => {
                if self.check(&TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.eat(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.eat(&TokenKind::Comma) {
                                continue;
                            }
                            self.expect(&TokenKind::RParen)?;
                            break;
                        }
                    }
                    Expr::Function {
                        name: name.to_lowercase(),
                        args,
                    }
                } else {
                    match name.to_ascii_lowercase().as_str() {
                        "true" => Expr::Literal(Value::Bool(true)),
                        "false" => Expr::Literal(Value::Bool(false)),
                        "null" => Expr::Literal(Value::Null),
                        _ => Expr::Variable(name.clone()),
                    }
                }
            }
            other => {
                return Err(self.error_at(
                    &token,
                    format!("expected an expression, found {}", other.describe()),
                ))
            }
        };

        while self.eat(&TokenKind::Dot) {
            let key = self.expect_ident("a property name")?;
            expr = Expr::Property(Box::new(expr), key);
        }

        Ok(expr)
    }

    fn expect_ident(&mut self, what: &str) -> Result<String> {
        let token = self.advance();
        match &token.kind {
            TokenKind::Ident(name) => Ok(name.clone()),
            other => Err(self.error_at(
                &token,
                format!("expected {what}, found {}", other.describe()),
            )),
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<()> {
        let token = self.advance();
        if &token.kind == kind {
            Ok(())
        } else {
            Err(self.error_at(
                &token,
                format!(
                    "expected {}, found {}",
                    kind.describe(),
                    token.kind.describe()
                ),
            ))
        }
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn check_keyword(&self, keyword: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(name) if name.eq_ignore_ascii_case(keyword))
    }

    fn eat_keyword(&mut self, keyword: &str) -> bool {
        if self.check_keyword(keyword) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<()> {
        if self.eat_keyword(keyword) {
            Ok(())
        } else {
            let token = self.peek().clone();
            Err(self.error_at(
                &token,
                format!("expected {}, found {}", keyword, token.kind.describe()),
            ))
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) -> Token {
        let token = self.tokens[self.position].clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.position += 1;
        }
        token
    }

    fn error_at(&self, token: &Token, message: impl Into<String>) -> Error {
        Error::at(message, token.line, token.column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_match_return() {
        let statement = parse_statement(
            r#"MATCH (n:Person {name: "Alice"})-[:KNOWS]->(friend:Person)
               WHERE n.age >= 30 AND friend.name <> "Bob"
               RETURN friend.name AS name, count(*) ORDER BY name DESC LIMIT 5"#,
        )
        .unwrap();

        match statement {
            Statement::Match {
                patterns,
                where_clause,
                action,
            } => {
                assert_eq!(patterns.len(), 1);
                assert_eq!(patterns[0].chains.len(), 1);
                assert!(where_clause.is_some());
                match action {
                    MatchAction::Return(ret) => {
                        assert_eq!(ret.items.len(), 2);
                        assert_eq!(ret.order_by.len(), 1);
                        assert!(ret.limit.is_some());
                    }
                    other => panic!("unexpected action: {other:?}"),
                }
            }
            other => panic!("unexpected statement: {other:?}"),
        }
    }

    #[test]
    fn parses_create_and_delete() {
        parse_statement(r#"CREATE (a:Person {name: "A"})-[:KNOWS]->(b:Person)"#).unwrap();
        parse_statement("MATCH (n:Person) DETACH DELETE n").unwrap();
        parse_statement("MATCH (n:Person) SET n.age = n.age + 1").unwrap();
    }

    #[test]
    fn rejects_incomplete_statement() {
        let error = parse_statement("MATCH (n)").unwrap_err();
        assert!(error.message.contains("must be followed"));
    }
}
