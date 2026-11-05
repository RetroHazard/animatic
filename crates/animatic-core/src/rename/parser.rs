//! Lexer, preprocessor and right-associative recursive-descent parser of the rename language.

use super::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Literal(Value),
    /// `left` is `None` for unary minus (evaluated as `False - right`).
    Binary(BinOp, Option<Box<Node>>, Box<Node>),
    Not(Box<Node>),
    /// Variable, bound object or property (`A`, `A.Name`, `atitle`).
    Symbol {
        parent: Option<Box<Node>>,
        name: String,
    },
    /// Built-in, application or user function call.
    Call {
        parent: Option<Box<Node>>,
        name: String,
        args: Vec<Node>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Plus,
    And,
    Comma,
    False,
    Number(f64),
    Str(String),
    True,
    Slash,
    Dot,
    Eq,
    Ge,
    Gt,
    Div,
    Le,
    LParen,
    Lt,
    Mod,
    Star,
    Ne,
    Eof,
    Not,
    Or,
    RParen,
    Minus,
    Ident(String),
    Xor,
}

/// Compile error with the legacy context decoration.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Preprocess a multi-line pattern into one expression: drop blank and `//` lines, join
/// lines ending in `_`, wrap every line in parentheses and join with ` + `.
pub fn preprocess(src: &str) -> String {
    let mut lines: Vec<String> = src
        .split("\r\n")
        .flat_map(|l| l.split(['\r', '\n']))
        .map(str::to_string)
        .collect();
    for i in (0..lines.len()).rev() {
        let t = lines[i].trim_matches(|c: char| c <= ' ').to_string();
        if t.is_empty() || t.starts_with("//") {
            lines.remove(i);
        } else {
            lines[i] = t;
        }
    }
    if lines.len() > 1 {
        for i in (0..lines.len() - 1).rev() {
            if lines[i].ends_with('_') {
                lines[i].pop();
                let next = lines.remove(i + 1);
                lines[i].push_str(&next);
            }
        }
    }
    lines
        .iter()
        .map(|l| format!("({l})"))
        .collect::<Vec<_>>()
        .join(" + ")
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    current: Tok,
    pushed_back: bool,
}

impl Lexer {
    fn new(src: &str) -> Lexer {
        Lexer {
            chars: src.chars().collect(),
            pos: 0,
            current: Tok::Eof,
            pushed_back: false,
        }
    }

    fn unget(&mut self) {
        self.pushed_back = true;
    }

    fn next(&mut self) -> Tok {
        if self.pushed_back {
            self.pushed_back = false;
            return self.current.clone();
        }
        self.current = self.scan();
        self.current.clone()
    }

    fn peek_char(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn scan(&mut self) -> Tok {
        while matches!(self.peek_char(0), Some(' ' | '\n' | '\x0c')) {
            self.pos += 1;
        }
        let Some(c) = self.peek_char(0) else {
            return Tok::Eof;
        };
        if c.is_ascii_digit() {
            let start = self.pos;
            while matches!(self.peek_char(0), Some(d) if d.is_ascii_digit() || d == '.') {
                self.pos += 1;
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            return Tok::Number(text.parse().unwrap_or(0.0));
        }
        if c.is_ascii_alphabetic() {
            let start = self.pos;
            while matches!(self.peek_char(0), Some(d) if d.is_ascii_alphanumeric() || d == '_') {
                self.pos += 1;
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            return match text.to_ascii_uppercase().as_str() {
                "AND" => Tok::And,
                "OR" => Tok::Or,
                "XOR" => Tok::Xor,
                "NOT" => Tok::Not,
                "TRUE" => Tok::True,
                "FALSE" => Tok::False,
                "MOD" => Tok::Mod,
                "DIV" => Tok::Div,
                _ => Tok::Ident(text),
            };
        }
        let two = |l: &Lexer| l.peek_char(1);
        let tok = match c {
            '+' => Tok::Plus,
            '-' => Tok::Minus,
            '*' => Tok::Star,
            '/' => Tok::Slash,
            '.' => Tok::Dot,
            '=' => Tok::Eq,
            ',' => Tok::Comma,
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '<' => match two(self) {
                Some('>') => {
                    self.pos += 1;
                    Tok::Ne
                }
                Some('=') => {
                    self.pos += 1;
                    Tok::Le
                }
                _ => Tok::Lt,
            },
            '>' => match two(self) {
                Some('=') => {
                    self.pos += 1;
                    Tok::Ge
                }
                _ => Tok::Gt,
            },
            '"' | '\'' => {
                self.pos += 1;
                let start = self.pos;
                while matches!(self.peek_char(0), Some(d) if d != c) {
                    self.pos += 1;
                }
                let text: String = self.chars[start..self.pos].iter().collect();
                if self.peek_char(0).is_some() {
                    self.pos += 1;
                }
                return Tok::Str(text);
            }
            // Unknown characters end the token stream without advancing (legacy behaviour).
            _ => return Tok::Eof,
        };
        self.pos += 1;
        tok
    }
}

const E_PAREN: &str = "TRuleEvaluator.GetExpression7: tkParLeft: Missing right parentesis!";
const E_NAME: &str = "TRuleEvaluator.GetExpression7: tkSymbol: Property or method name missing!";
const E_CALL_PAREN: &str = "TRuleEvaluator.GetExpression7: tkSymbol: Missing right parentesis!";
const E_RIGHT: &str = "TRuleEvaluator.GetExpression7: Right part of expression missing!";
const E_SYNTAX: &str = "TRuleManager.Compile: Syntax error!";

struct Parser {
    lx: Lexer,
}

type PResult = Result<Node, &'static str>;

impl Parser {
    fn binary(&mut self, level: u8) -> PResult {
        if level == 7 {
            return self.primary();
        }
        let left = self.binary(level + 1)?;
        let tok = self.lx.next();
        let op = match (level, &tok) {
            (1, Tok::Or) => Some(BinOp::Or),
            (2, Tok::And) => Some(BinOp::And),
            (3, Tok::Xor) => Some(BinOp::Xor),
            (4, Tok::Eq) => Some(BinOp::Eq),
            (4, Tok::Ne) => Some(BinOp::Ne),
            (4, Tok::Lt) => Some(BinOp::Lt),
            (4, Tok::Le) => Some(BinOp::Le),
            (4, Tok::Gt) => Some(BinOp::Gt),
            (4, Tok::Ge) => Some(BinOp::Ge),
            (5, Tok::Plus) => Some(BinOp::Add),
            (5, Tok::Minus) => Some(BinOp::Sub),
            (6, Tok::Star) => Some(BinOp::Mul),
            (6, Tok::Slash) => Some(BinOp::Div),
            (6, Tok::Div) => Some(BinOp::IntDiv),
            (6, Tok::Mod) => Some(BinOp::Mod),
            _ => None,
        };
        match op {
            Some(op) => {
                // Right operand recurses into the same level: operators are right-associative.
                let right = self.binary(level)?;
                Ok(Node::Binary(op, Some(Box::new(left)), Box::new(right)))
            }
            None => {
                self.lx.unget();
                Ok(left)
            }
        }
    }

    fn expr(&mut self) -> PResult {
        self.binary(1)
    }

    fn primary(&mut self) -> PResult {
        match self.lx.next() {
            Tok::Number(n) => Ok(Node::Literal(Value::Double(n))),
            Tok::Str(s) => Ok(Node::Literal(Value::WStr(s))),
            Tok::True => Ok(Node::Literal(Value::Bool(true))),
            Tok::False => Ok(Node::Literal(Value::Bool(false))),
            Tok::LParen => {
                let e = self.expr()?;
                if self.lx.next() != Tok::RParen {
                    return Err(E_PAREN);
                }
                Ok(e)
            }
            Tok::Plus => self.primary(),
            Tok::Minus => Ok(Node::Binary(BinOp::Sub, None, Box::new(self.primary()?))),
            Tok::Not => Ok(Node::Not(Box::new(self.primary()?))),
            Tok::Ident(_) => {
                self.lx.unget();
                self.symbol_chain()
            }
            _ => Err(E_RIGHT),
        }
    }

    fn symbol_chain(&mut self) -> PResult {
        let mut node: Option<Node> = None;
        loop {
            let name = match self.lx.next() {
                Tok::Ident(n) => n,
                _ => return Err(E_NAME),
            };
            let next = self.lx.next();
            self.lx.unget();
            let parent = node.take().map(Box::new);
            node = Some(if next == Tok::LParen {
                Node::Call {
                    parent,
                    name,
                    args: Vec::new(),
                }
            } else {
                Node::Symbol { parent, name }
            });
            if self.lx.next() != Tok::Dot {
                self.lx.unget();
                break;
            }
        }
        let mut node = node.expect("chain has at least one element");
        if self.lx.next() == Tok::LParen {
            let mut args = Vec::new();
            if self.lx.next() != Tok::RParen {
                self.lx.unget();
                loop {
                    args.push(self.expr()?);
                    match self.lx.next() {
                        Tok::Comma => continue,
                        Tok::RParen => break,
                        _ => return Err(E_CALL_PAREN),
                    }
                }
            }
            match &mut node {
                Node::Call { args: a, .. } => *a = args,
                _ => {
                    return Err("TRuleEvaluator.GetItem: Can't add parameters without a function!");
                }
            }
        } else {
            self.lx.unget();
        }
        Ok(node)
    }
}

/// Compile a preprocessed expression. `Ok(None)` for an empty text.
pub fn compile(text: &str) -> Result<Option<Node>, CompileError> {
    let text = text.trim_matches(|c: char| c <= ' ');
    if text.is_empty() {
        return Ok(None);
    }
    let mut p = Parser {
        lx: Lexer::new(text),
    };
    let result = p.expr().and_then(|n| {
        if p.lx.next() == Tok::Eof {
            Ok(n)
        } else {
            Err(E_SYNTAX)
        }
    });
    match result {
        Ok(n) => Ok(Some(n)),
        Err(msg) => {
            let chars: Vec<char> = text.chars().collect();
            let pos = p.lx.pos.min(chars.len());
            let start = pos.saturating_sub(15);
            let context: String = chars[start..(start + 31).min(chars.len())].iter().collect();
            let near: String = chars[pos..(pos + 16).min(chars.len())].iter().collect();
            Err(CompileError {
                message: format!("{msg}\r\nIn \"{context}...\" near \"{near}\""),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_wraps_lines_and_handles_comments_and_continuations() {
        let src = "set('a', 1)\r\n\r\n// comment\r\n  if(x,_\n'y')  \nfilename";
        assert_eq!(preprocess(src), "(set('a', 1)) + (if(x,'y')) + (filename)");
    }

    #[test]
    fn binary_operators_are_right_associative() {
        let n = compile("10-3-2").unwrap().unwrap();
        match n {
            Node::Binary(BinOp::Sub, Some(l), r) => {
                assert_eq!(*l, Node::Literal(Value::Double(10.0)));
                assert!(matches!(*r, Node::Binary(BinOp::Sub, ..)));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn calls_and_property_chains() {
        let n = compile("if(H, H.Crc32, F.Crc)").unwrap().unwrap();
        let Node::Call { name, args, parent } = n else {
            panic!()
        };
        assert_eq!(name, "if");
        assert!(parent.is_none());
        assert_eq!(args.len(), 3);
        assert_eq!(
            args[1],
            Node::Symbol {
                parent: Some(Box::new(Node::Symbol {
                    parent: None,
                    name: "H".into()
                })),
                name: "Crc32".into()
            }
        );
    }

    #[test]
    fn errors_carry_legacy_context() {
        let e = compile("('a'+'b'&'c')").unwrap_err();
        assert_eq!(
            e.message,
            "TRuleEvaluator.GetExpression7: tkParLeft: Missing right parentesis!\r\nIn \"('a'+'b'&'c')...\" near \"&'c')\""
        );
        let e = compile("(1))").unwrap_err();
        assert!(
            e.message.starts_with("TRuleManager.Compile: Syntax error!"),
            "{}",
            e.message
        );
        assert!(
            compile("getanime('B',1).Name").is_err(),
            "a call cannot be followed by '.'"
        );
    }

    #[test]
    fn strings_have_no_escapes_and_may_be_unterminated() {
        assert_eq!(
            compile("'\\'").unwrap(),
            Some(Node::Literal(Value::WStr("\\".into())))
        );
        assert_eq!(
            compile("\"it's\"").unwrap(),
            Some(Node::Literal(Value::WStr("it's".into())))
        );
        assert!(compile("('abc)").is_err());
    }
}
