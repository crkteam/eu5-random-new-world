//! Paradox 腳本格式（Clausewitz/Jomini）的最小解析器。
//!
//! 語法：`key = value`、`key = { ... }`、區塊裡也可以只放值（`{ a b c }`），
//! `#` 到行尾是註解，字串可以用雙引號。比較運算子（`<`、`>=`、`?=` 等）也當作賦值處理，
//! 並保留原本的運算子。

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Scalar(String),
    Block(Vec<Item>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// `key op value`
    Pair { key: String, op: Op, value: Value },
    /// 區塊中單獨出現的值，例如地點清單 `{ stockholm uppsala }`。
    Bare(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
    /// `?=`：存在才比較。
    Maybe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Quoted(String),
    Open,
    Close,
    Op(Op),
}

fn tokenize(text: &str) -> Result<Vec<(Token, usize)>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    let mut line = 1;
    while let Some(&c) = chars.peek() {
        match c {
            '\n' => {
                line += 1;
                chars.next();
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            '#' => {
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '{' => {
                chars.next();
                tokens.push((Token::Open, line));
            }
            '}' => {
                chars.next();
                tokens.push((Token::Close, line));
            }
            '=' | '<' | '>' | '!' | '?' => {
                chars.next();
                let followed_by_eq = chars.peek() == Some(&'=');
                let op = match (c, followed_by_eq) {
                    ('=', _) => Op::Eq,
                    ('<', true) => Op::Le,
                    ('<', false) => Op::Lt,
                    ('>', true) => Op::Ge,
                    ('>', false) => Op::Gt,
                    ('!', true) => Op::Ne,
                    ('?', true) => Op::Maybe,
                    _ => {
                        return Err(ParseError { line, message: format!("unexpected '{c}'") });
                    }
                };
                if followed_by_eq && c != '=' {
                    chars.next();
                }
                tokens.push((Token::Op(op), line));
            }
            '"' => {
                chars.next();
                let start = line;
                let mut text = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped) => text.push(escaped),
                            None => break,
                        },
                        Some(c) => {
                            if c == '\n' {
                                line += 1;
                            }
                            text.push(c);
                        }
                        None => {
                            return Err(ParseError {
                                line: start,
                                message: "unterminated string".into(),
                            });
                        }
                    }
                }
                tokens.push((Token::Quoted(text), start));
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || "{}=<>!?#\"".contains(c) {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                tokens.push((Token::Word(word), line));
            }
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    at: usize,
}

impl Parser {
    fn line(&self) -> usize {
        self.tokens.get(self.at).or(self.tokens.last()).map_or(1, |t| t.1)
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError { line: self.line(), message: message.into() }
    }

    /// 解析區塊內容直到 `}`（`nested`）或檔案結尾。
    fn items(&mut self, nested: bool) -> Result<Vec<Item>, ParseError> {
        let mut items = Vec::new();
        loop {
            let Some((token, _)) = self.tokens.get(self.at).cloned() else {
                if nested {
                    return Err(self.error("missing '}'"));
                }
                return Ok(items);
            };
            self.at += 1;
            match token {
                Token::Close if nested => return Ok(items),
                Token::Close => return Err(self.error("unexpected '}'")),
                Token::Op(_) => return Err(self.error("operator without a key")),
                Token::Open => items.push(Item::Bare(Value::Block(self.items(true)?))),
                Token::Word(key) | Token::Quoted(key) => {
                    if let Some((Token::Op(op), _)) = self.tokens.get(self.at).cloned() {
                        self.at += 1;
                        let value = self.value()?;
                        items.push(Item::Pair { key, op, value });
                    } else {
                        items.push(Item::Bare(Value::Scalar(key)));
                    }
                }
            }
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        let Some((token, _)) = self.tokens.get(self.at).cloned() else {
            return Err(self.error("missing value"));
        };
        self.at += 1;
        match token {
            Token::Word(text) | Token::Quoted(text) => Ok(Value::Scalar(text)),
            Token::Open => Ok(Value::Block(self.items(true)?)),
            Token::Close | Token::Op(_) => Err(self.error("missing value")),
        }
    }
}

/// 解析整份檔案。
pub fn parse(text: &str) -> Result<Vec<Item>, ParseError> {
    let mut parser = Parser { tokens: tokenize(text)?, at: 0 };
    parser.items(false)
}

/// 取得 `key = 純量` 的值（第一個符合的）。
pub fn scalar<'a>(items: &'a [Item], wanted: &str) -> Option<&'a str> {
    items.iter().find_map(|item| match item {
        Item::Pair { key, value: Value::Scalar(text), .. } if key == wanted => Some(text.as_str()),
        _ => None,
    })
}

/// 取得 `key = { ... }` 的內容（第一個符合的）。
pub fn block<'a>(items: &'a [Item], wanted: &str) -> Option<&'a [Item]> {
    items.iter().find_map(|item| match item {
        Item::Pair { key, value: Value::Block(inner), .. } if key == wanted => Some(inner.as_slice()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Value {
        Value::Scalar(text.into())
    }

    #[test]
    fn pairs_blocks_and_bare_values() {
        let items = parse("a = 1 # 註解\nb = { x y }\nc = \"hello world\"").unwrap();
        assert_eq!(scalar(&items, "a"), Some("1"));
        assert_eq!(block(&items, "b").unwrap(), &[Item::Bare(s("x")), Item::Bare(s("y"))]);
        assert_eq!(scalar(&items, "c"), Some("hello world"));
    }

    #[test]
    fn comparison_operators() {
        let items = parse("x >= 3 y < 2 z ?= w").unwrap();
        let ops: Vec<Op> = items
            .iter()
            .map(|i| match i {
                Item::Pair { op, .. } => *op,
                Item::Bare(_) => panic!("bare"),
            })
            .collect();
        assert_eq!(ops, [Op::Ge, Op::Lt, Op::Maybe]);
    }

    #[test]
    fn errors_name_the_line() {
        assert_eq!(parse("a = {\n b = 1\n").unwrap_err().message, "missing '}'");
        assert_eq!(parse("}\n").unwrap_err().line, 1);
        assert_eq!(parse("a =").unwrap_err().message, "missing value");
    }
}
