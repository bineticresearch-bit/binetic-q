//! Hand-rolled lexer for `.q` source. No regex dependency; every byte is
//! classified explicitly so the grammar stays auditable.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Ident(String),
    Int(i64),
    At,
    Arrow,
    Pipe,
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Eq,
    Colon,
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    UnexpectedChar { ch: char, pos: usize },
    BadInteger { text: String, pos: usize },
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LexError::UnexpectedChar { ch, pos } => {
                write!(f, "unexpected char {ch:?} at byte {pos}")
            }
            LexError::BadInteger { text, pos } => write!(f, "bad integer {text:?} at byte {pos}"),
        }
    }
}

impl std::error::Error for LexError {}

#[derive(Debug, Clone)]
pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.bytes.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_trivia(&mut self) {
        loop {
            while matches!(self.peek(), Some(b) if b.is_ascii_whitespace()) {
                self.pos += 1;
            }
            if self.peek() == Some(b'/') && self.peek2() == Some(b'/') {
                while let Some(b) = self.bump() {
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn ident(&mut self, start: usize) -> Token {
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        Token::Ident(self.src[start..self.pos].to_string())
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            let start = self.pos;
            let b = match self.bump() {
                Some(b) => b,
                None => {
                    out.push(Token::Eof);
                    break;
                }
            };
            match b {
                b'@' => out.push(Token::At),
                b'{' => out.push(Token::LBrace),
                b'}' => out.push(Token::RBrace),
                b'(' => out.push(Token::LParen),
                b')' => out.push(Token::RParen),
                b'[' => out.push(Token::LBracket),
                b']' => out.push(Token::RBracket),
                b',' => out.push(Token::Comma),
                b';' => out.push(Token::Semi),
                b'=' => out.push(Token::Eq),
                b':' => out.push(Token::Colon),
                b'-' if self.peek() == Some(b'>') => {
                    self.pos += 1;
                    out.push(Token::Arrow);
                }
                b'|' if self.peek() == Some(b'>') => {
                    self.pos += 1;
                    out.push(Token::Pipe);
                }
                b'0'..=b'9' => {
                    while matches!(self.peek(), Some(d) if d.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    let text = &self.src[start..self.pos];
                    let v = text.parse::<i64>().map_err(|_| LexError::BadInteger {
                        text: text.to_string(),
                        pos: start,
                    })?;
                    out.push(Token::Int(v));
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    out.push(self.ident(start));
                }
                other => {
                    return Err(LexError::UnexpectedChar {
                        ch: other as char,
                        pos: start,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_a_pipeline_program() {
        let src = "@receipt L2\nqblock p(x: i64[8]) -> i64[8] {\n  let h = dot(x, 3);\n  let y = h |> gate(9);\n  emit y;\n}\n";
        let toks = Lexer::new(src).tokenize().unwrap();
        assert!(toks.contains(&Token::At));
        assert!(toks.contains(&Token::Pipe));
        assert!(toks.contains(&Token::Arrow));
        assert!(toks.contains(&Token::Int(3)));
        assert_eq!(toks.last(), Some(&Token::Eof));
    }

    #[test]
    fn rejects_unexpected_character() {
        let err = Lexer::new("qblock $").tokenize().unwrap_err();
        assert!(matches!(err, LexError::UnexpectedChar { ch: '$', .. }));
    }
}
