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

use crate::ir::{DType, Kernel, Level, ProgramConfig, QProgram, Stage, TensorType};
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

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub fn parse(src: &str) -> Result<QProgram, ParseError> {
        let tokens = crate::lexer::Lexer::new(src).tokenize()?;
        let mut p = Parser::new(tokens);
        p.program()
    }

    fn cur(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        self.pos += 1;
        t
    }

    fn expect(&mut self, want: Token, label: &str) -> Result<(), ParseError> {
        let got = self.bump();
        if got == want {
            Ok(())
        } else {
            Err(ParseError::Unexpected {
                expected: label.to_string(),
                found: format!("{got:?}"),
            })
        }
    }

    fn ident(&mut self) -> Result<String, ParseError> {
        match self.bump() {
            Token::Ident(s) => Ok(s),
            other => Err(ParseError::Unexpected {
                expected: "identifier".to_string(),
                found: format!("{other:?}"),
            }),
        }
    }

    fn uint(&mut self) -> Result<u64, ParseError> {
        match self.bump() {
            Token::Int(n) if n >= 0 => Ok(n as u64),
            other => Err(ParseError::Unexpected {
                expected: "non-negative integer".to_string(),
                found: format!("{other:?}"),
            }),
        }
    }

    fn ty(&mut self) -> Result<TensorType, ParseError> {
        let name = self.ident()?;
        let dtype = DType::parse(&name).ok_or_else(|| ParseError::BadType(name.clone()))?;
        self.expect(Token::LBracket, "'['")?;
        let n = self.uint()? as usize;
        self.expect(Token::RBracket, "']'")?;
        Ok(TensorType::new(dtype, vec![n]))
    }

    fn config(&mut self) -> Result<ProgramConfig, ParseError> {
        self.expect(Token::Ident("config".to_string()), "'config'")?;
        self.expect(Token::LBrace, "'{'")?;
        let mut cfg = ProgramConfig::default();
        while matches!(self.cur(), Token::Ident(_)) {
            let key = self.ident()?;
            self.expect(Token::Eq, "'='")?;
            let val = self.uint()?;
            self.expect(Token::Semi, "';'")?;
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
        self.expect(Token::RBrace, "'}'")?;
        Ok(cfg)
    }

    fn arg(&mut self) -> Result<(Option<String>, i64), ParseError> {
        match self.bump() {
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
        let name = self.ident()?;
        let kernel = Kernel::parse(&name);
        self.expect(Token::LParen, "'('")?;
        let mut inputs = Vec::new();
        let mut weight = 0i64;
        if *self.cur() != Token::RParen {
            loop {
                let (ident, num) = self.arg()?;
                match ident {
                    Some(s) => inputs.push(s),
                    None => weight = num,
                }
                if *self.cur() == Token::Comma {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(Token::RParen, "')'")?;
        Ok((kernel, inputs, weight))
    }

    fn expr(&mut self) -> Result<(Kernel, Vec<String>, i64), ParseError> {
        // Either a direct call, or `IDENT |> call` (piped value becomes first input).
        if let Token::Ident(name) = self.cur().clone() {
            if self.tokens.get(self.pos + 1) == Some(&Token::Pipe) {
                self.bump(); // piped ident
                self.bump(); // '|>'
                let (kernel, mut inputs, weight) = self.call()?;
                inputs.insert(0, name);
                return Ok((kernel, inputs, weight));
            }
        }
        self.call()
    }

    fn program(&mut self) -> Result<QProgram, ParseError> {
        self.expect(Token::At, "'@'")?;
        let receipt_kw = self.ident()?;
        if receipt_kw != "receipt" {
            return Err(ParseError::Unexpected {
                expected: "'receipt'".to_string(),
                found: receipt_kw,
            });
        }
        let level_name = self.ident()?;
        let demand = Level::parse(&level_name).ok_or_else(|| ParseError::BadReceipt(level_name))?;

        self.expect(Token::Ident("qblock".to_string()), "'qblock'")?;
        let name = self.ident()?;
        self.expect(Token::LParen, "'('")?;
        let param = self.ident()?;
        self.expect(Token::Colon, "':'")?;
        let param_ty = self.ty()?;
        self.expect(Token::RParen, "')'")?;
        self.expect(Token::Arrow, "'->'")?;
        let output_ty = self.ty()?;
        self.expect(Token::LBrace, "'{'")?;

        let config = self.config()?;
        let mut stages = Vec::new();
        while *self.cur() == Token::Ident("let".to_string()) {
            self.bump(); // 'let'
            let sname = self.ident()?;
            self.expect(Token::Eq, "'='")?;
            let (kernel, inputs, weight) = self.expr()?;
            self.expect(Token::Semi, "';'")?;
            stages.push(Stage {
                name: sname,
                kernel,
                inputs,
                weight,
            });
        }
        self.expect(Token::Ident("emit".to_string()), "'emit'")?;
        let emit = self.ident()?;
        self.expect(Token::Semi, "';'")?;
        self.expect(Token::RBrace, "'}'")?;
        self.expect(Token::Eof, "end of file")?;

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
