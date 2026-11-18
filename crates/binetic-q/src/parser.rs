//! Recursive-descent parser for `.q` source → [`QProgram`].
//!
//! Grammar (Q0):
//! ```text
//! program    := "@receipt" LEVEL "qblock" IDENT "(" param ")" "->" type "{" body "}"
//! param      := IDENT ":" type
//! type       := ("i64" | "f32") "[" INT "]"
//! body       := config_decl stmt* emit_stmt
//! config_decl:= "config" "{" field* "}"
//! field      := IDENT "=" INT ";"
//! stmt       := "let" IDENT "=" expr ";"
//! expr       := call | IDENT "|>" call
//! call       := IDENT "(" args? ")"
//! args       := (IDENT | INT) ("," (IDENT | INT))*
//! emit_stmt  := "emit" IDENT ";"
//! ```
//! Pipeline sugar: `h |> dot(5)` desugars to `dot(h, 5)` (piped value first).

use crate::cursor::TokenCursor;
use crate::ir::{Kernel, Level, ProgramConfig, QProgram, Stage};
use crate::lexer::{LexError, Token};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Lex(LexError),
    Unexpected { expected: String, found: String },
    BadReceipt(String),
    BadType(String),
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError::Lex(e)
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Lex(e) => write!(f, "lex error: {e}"),
            ParseError::Unexpected { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            ParseError::BadReceipt(s) => write!(f, "bad receipt level {s:?}"),
            ParseError::BadType(s) => write!(f, "bad type {s:?}"),
        }
    }
}

impl std::error::Error for ParseError {}

fn unexpected(expected: &str, found: &str) -> ParseError {
    ParseError::Unexpected {
        expected: expected.to_string(),
        found: found.to_string(),
    }
}

pub struct Parser {
    c: TokenCursor<ParseError>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            c: TokenCursor::new(tokens, unexpected),
        }
    }

    pub fn parse(src: &str) -> Result<QProgram, ParseError> {
        let tokens = crate::lexer::Lexer::new(src).tokenize()?;
        let mut p = Parser::new(tokens);
        p.program()
    }

    fn config(&mut self) -> Result<ProgramConfig, ParseError> {
        self.c.expect(Token::Ident("config".to_string()), "'config'")?;
        self.c.expect(Token::LBrace, "'{'")?;
        let mut cfg = ProgramConfig::default();
        while matches!(self.c.cur(), Token::Ident(_)) {
            let key = self.c.ident("config field")?;
            self.c.expect(Token::Eq, "'='")?;
            let val = self.c.uint("integer value")?;
            self.c.expect(Token::Semi, "';'")?;
            match key.as_str() {
                "model_id" => cfg.model_id = val as u32,
                "lane_width" => cfg.lane_width = val as u16,
                "tile_size" => cfg.tile_size = val as u32,
                "node_count" => cfg.node_count = val as u32,
                "seed" => cfg.seed = val,
                _ => {
                    return Err(ParseError::Unexpected {
                        expected: "config field".to_string(),
                        found: key,
                    })
                }
            }
        }
        self.c.expect(Token::RBrace, "'}'")?;
        Ok(cfg)
    }

    fn arg(&mut self) -> Result<(Option<String>, i64), ParseError> {
        match self.c.bump() {
            Token::Ident(s) => Ok((Some(s), 0)),
            Token::Int(n) => Ok((None, n)),
            other => Err(ParseError::Unexpected {
                expected: "identifier or integer argument".to_string(),
                found: format!("{other:?}"),
            }),
        }
    }

    /// Returns (kernel, named inputs, weight scalar).
    fn call(&mut self) -> Result<(Kernel, Vec<String>, i64), ParseError> {
        let name = self.c.ident("kernel name")?;
        let kernel = Kernel::parse(&name);
        self.c.expect(Token::LParen, "'('")?;
        let mut inputs = Vec::new();
        let mut weight = 0i64;
        if *self.c.cur() != Token::RParen {
            loop {
                let (ident, num) = self.arg()?;
                match ident {
                    Some(s) => inputs.push(s),
                    None => weight = num,
                }
                if *self.c.cur() == Token::Comma {
                    self.c.bump();
                    continue;
                }
                break;
            }
        }
        self.c.expect(Token::RParen, "')'")?;
        Ok((kernel, inputs, weight))
    }

    fn expr(&mut self) -> Result<(Kernel, Vec<String>, i64), ParseError> {
        // Either a direct call, or `IDENT |> call` (piped value becomes first input).
        if let Token::Ident(name) = self.c.cur().clone() {
            if self.c.tokens.get(self.c.pos + 1) == Some(&Token::Pipe) {
                self.c.bump(); // piped ident
                self.c.bump(); // '|>'
                let (kernel, mut inputs, weight) = self.call()?;
                inputs.insert(0, name);
                return Ok((kernel, inputs, weight));
            }
        }
        self.call()
    }

    fn program(&mut self) -> Result<QProgram, ParseError> {
        self.c.expect(Token::At, "'@'")?;
        let receipt_kw = self.c.ident("'receipt'")?;
        if receipt_kw != "receipt" {
            return Err(ParseError::Unexpected {
                expected: "'receipt'".to_string(),
                found: receipt_kw,
            });
        }
        let level_name = self.c.ident("receipt level")?;
        let demand =
            Level::parse(&level_name).ok_or_else(|| ParseError::BadReceipt(level_name))?;

        self.c.expect(Token::Ident("qblock".to_string()), "'qblock'")?;
        let name = self.c.ident("block name")?;
        self.c.expect(Token::LParen, "'('")?;
        let param = self.c.ident("parameter name")?;
        self.c.expect(Token::Colon, "':'")?;
        let param_ty = self.c.ty(ParseError::BadType)?;
        self.c.expect(Token::RParen, "')'")?;
        self.c.expect(Token::Arrow, "'->'")?;
        let output_ty = self.c.ty(ParseError::BadType)?;
        self.c.expect(Token::LBrace, "'{'")?;

        let config = self.config()?;
        let mut stages = Vec::new();
        while *self.c.cur() == Token::Ident("let".to_string()) {
            self.c.bump(); // 'let'
            let sname = self.c.ident("stage name")?;
            self.c.expect(Token::Eq, "'='")?;
            let (kernel, inputs, weight) = self.expr()?;
            self.c.expect(Token::Semi, "';'")?;
            stages.push(Stage {
                name: sname,
                kernel,
                inputs,
                weight,
            });
        }
        self.c.expect(Token::Ident("emit".to_string()), "'emit'")?;
        let emit = self.c.ident("emit name")?;
        self.c.expect(Token::Semi, "';'")?;
        self.c.expect(Token::RBrace, "'}'")?;
        self.c.expect(Token::Eof, "end of file")?;

        Ok(QProgram {
            name,
            param,
            param_ty,
            output_ty,
            stages,
            emit,
            demand,
            config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = r#"
@receipt L2
qblock pipeline(x: i64[8]) -> i64[8] {
  config {
    model_id = 11;
    lane_width = 8;
    tile_size = 8;
    node_count = 2;
    seed = 7;
  }
  let h = dot(x, 3);
  let g = h |> dot(5);
  let y = g |> gate(9);
  emit y;
}
"#;

    #[test]
    fn parses_pipeline_sugar() {
        let p = Parser::parse(P).unwrap();
        assert_eq!(p.name, "pipeline");
        assert_eq!(p.demand, Level::L2);
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.stages[1].inputs, vec!["h"]);
        assert_eq!(p.stages[1].weight, 5);
        assert_eq!(p.emit, "y");
        assert_eq!(p.config.model_id, 11);
        assert_eq!(p.config.seed, 7);
    }

    #[test]
    fn rejects_unknown_config_field() {
        let bad = P.replace("model_id = 11;", "bogus = 11;");
        assert!(Parser::parse(&bad).is_err());
    }
}
