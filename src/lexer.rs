use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    String(String),
    Int(i64),
    Float(f64),

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Dot,
    Semicolon,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Minus,
    ArrowLeft,
    ArrowRight,
    Plus,
    Star,
    Slash,
    Percent,
    Eof,
}

impl TokenKind {
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(value) => format!("`{value}`"),
            TokenKind::String(_) => "a string".to_string(),
            TokenKind::Int(value) => format!("`{value}`"),
            TokenKind::Float(value) => format!("`{value}`"),
            TokenKind::LParen => "`(`".to_string(),
            TokenKind::RParen => "`)`".to_string(),
            TokenKind::LBrace => "`{`".to_string(),
            TokenKind::RBrace => "`}`".to_string(),
            TokenKind::LBracket => "`[`".to_string(),
            TokenKind::RBracket => "`]`".to_string(),
            TokenKind::Colon => "`:`".to_string(),
            TokenKind::Comma => "`,`".to_string(),
            TokenKind::Dot => "`.`".to_string(),
            TokenKind::Semicolon => "`;`".to_string(),
            TokenKind::Eq => "`=`".to_string(),
            TokenKind::NotEq => "`<>`".to_string(),
            TokenKind::Lt => "`<`".to_string(),
            TokenKind::LtEq => "`<=`".to_string(),
            TokenKind::Gt => "`>`".to_string(),
            TokenKind::GtEq => "`>=`".to_string(),
            TokenKind::Minus => "`-`".to_string(),
            TokenKind::ArrowLeft => "`<-`".to_string(),
            TokenKind::ArrowRight => "`->`".to_string(),
            TokenKind::Plus => "`+`".to_string(),
            TokenKind::Star => "`*`".to_string(),
            TokenKind::Slash => "`/`".to_string(),
            TokenKind::Percent => "`%`".to_string(),
            TokenKind::Eof => "end of input".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
}

pub fn lex(source: &str) -> Result<Vec<Token>> {
    Lexer::new(source).run()
}

struct Lexer {
    chars: Vec<char>,
    position: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Lexer {
            chars: source.chars().collect(),
            position: 0,
            line: 1,
            column: 1,
        }
    }

    fn run(mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments()?;
            if self.peek().is_none() {
                break;
            }

            let line = self.line;
            let column = self.column;
            let kind = self.next_token()?;
            tokens.push(Token { kind, line, column });
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            line: self.line,
            column: self.column,
        });
        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<TokenKind> {
        let ch = self.advance().expect("caller checked for eof");
        let kind = match ch {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            ';' => TokenKind::Semicolon,
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '<' => {
                if self.peek() == Some('-') {
                    self.advance();
                    TokenKind::ArrowLeft
                } else if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::LtEq
                } else if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::NotEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '-' => {
                if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::ArrowRight
                } else {
                    TokenKind::Minus
                }
            }
            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                }
                TokenKind::Eq
            }
            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::NotEq
                } else {
                    return Err(self.error("unexpected character `!`"));
                }
            }
            '"' | '\'' => self.string(ch)?,
            ch if ch.is_ascii_digit() => self.number(ch)?,
            ch if is_ident_start(ch) => self.identifier(ch),
            other => {
                return Err(self.error(format!("unexpected character `{other}`")));
            }
        };
        Ok(kind)
    }

    fn string(&mut self, quote: char) -> Result<TokenKind> {
        let mut value = String::new();
        loop {
            let ch = self
                .advance()
                .ok_or_else(|| self.error("unterminated string literal"))?;
            if ch == quote {
                return Ok(TokenKind::String(value));
            }
            if ch == '\\' {
                let escaped = self
                    .advance()
                    .ok_or_else(|| self.error("unterminated string literal"))?;
                match escaped {
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    '"' => value.push('"'),
                    '\'' => value.push('\''),
                    '\\' => value.push('\\'),
                    other => {
                        return Err(self.error(format!("unknown string escape `\\{other}`")));
                    }
                }
            } else {
                value.push(ch);
            }
        }
    }

    fn number(&mut self, first: char) -> Result<TokenKind> {
        let mut text = String::new();
        text.push(first);

        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || ch == '_' {
                text.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        let mut is_float = false;
        if self.peek() == Some('.') && self.peek_next().is_some_and(|ch| ch.is_ascii_digit()) {
            is_float = true;
            text.push('.');
            self.advance();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() || ch == '_' {
                    text.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            let save_position = self.position;
            let save_line = self.line;
            let save_column = self.column;
            let mut exponent = String::new();
            exponent.push(self.advance().expect("peeked"));
            if matches!(self.peek(), Some('+' | '-')) {
                exponent.push(self.advance().expect("peeked"));
            }
            if self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                while let Some(ch) = self.peek() {
                    if ch.is_ascii_digit() {
                        exponent.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
                text.push_str(&exponent);
                is_float = true;
            } else {
                self.position = save_position;
                self.line = save_line;
                self.column = save_column;
            }
        }

        let cleaned = text.replace('_', "");
        if is_float {
            cleaned
                .parse::<f64>()
                .map(TokenKind::Float)
                .map_err(|_| self.error(format!("invalid number `{text}`")))
        } else {
            cleaned
                .parse::<i64>()
                .map(TokenKind::Int)
                .map_err(|_| self.error(format!("integer `{text}` is out of range")))
        }
    }

    fn identifier(&mut self, first: char) -> TokenKind {
        let mut value = String::new();
        value.push(first);
        while let Some(ch) = self.peek() {
            if is_ident_continue(ch) {
                value.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        TokenKind::Ident(value)
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<()> {
        loop {
            while self.peek().is_some_and(|ch| ch.is_whitespace()) {
                self.advance();
            }

            if self.peek() == Some('/') && self.peek_next() == Some('/') {
                while let Some(ch) = self.advance() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }

            if self.peek() == Some('-') && self.peek_next() == Some('-') {
                // `--` starts a line comment, but only when it is not the
                // beginning of an arrow-like construct. A relationship is
                // written `-[...]->`, so `--` is safe to treat as a comment.
                while let Some(ch) = self.advance() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }

            break;
        }
        Ok(())
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.position + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.position).copied()?;
        self.position += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error::at(message, self.line, self.column)
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source)
            .unwrap()
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn lexes_a_query() {
        assert_eq!(
            kinds(r#"MATCH (n:Person {name: "A\"B"}) RETURN n.age >= 30"#),
            vec![
                TokenKind::Ident("MATCH".into()),
                TokenKind::LParen,
                TokenKind::Ident("n".into()),
                TokenKind::Colon,
                TokenKind::Ident("Person".into()),
                TokenKind::LBrace,
                TokenKind::Ident("name".into()),
                TokenKind::Colon,
                TokenKind::String("A\"B".into()),
                TokenKind::RBrace,
                TokenKind::RParen,
                TokenKind::Ident("RETURN".into()),
                TokenKind::Ident("n".into()),
                TokenKind::Dot,
                TokenKind::Ident("age".into()),
                TokenKind::GtEq,
                TokenKind::Int(30),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_arrows_and_comments() {
        assert_eq!(
            kinds("<-[:KNOWS]-> // comment\n -[r]-"),
            vec![
                TokenKind::ArrowLeft,
                TokenKind::LBracket,
                TokenKind::Colon,
                TokenKind::Ident("KNOWS".into()),
                TokenKind::RBracket,
                TokenKind::ArrowRight,
                TokenKind::Minus,
                TokenKind::LBracket,
                TokenKind::Ident("r".into()),
                TokenKind::RBracket,
                TokenKind::Minus,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reports_line_and_column() {
        let error = lex("MATCH (n)\nWHERE n.age @ 3").unwrap_err();
        assert_eq!(error.line, Some(2));
        assert_eq!(error.column, Some(14));
    }
}
