//! Shared token-cursor logic for the `.q` and `.intent` parsers.
//!
//! Both [`crate::parser::Parser`] and the intent parser in [`crate::synth`]
//! walk the same `Vec<Token>` stream with identical `cur`/`bump`/`ident`/
//! `uint`/`expect` primitives. `TokenCursor` factors out that shared machinery;
//! each caller supplies an error constructor so the cursor stays generic over
//! the calling parser's error type.

use crate::ir::{DType, TensorType};
use crate::lexer::Token;

/// A generic forward-only cursor over a token stream.
///
/// `E` is the calling parser's error type. The caller supplies `mk_err` — a
/// closure that maps `(expected_label, found_token_debug)` to `E` — so the
/// cursor's helpers can produce typed errors without knowing the error type.
pub struct TokenCursor<E> {
    pub tokens: Vec<Token>,
    pub pos: usize,
    mk_err: Box<dyn Fn(&str, &str) -> E>,
}

impl<E> TokenCursor<E> {
    pub fn new(tokens: Vec<Token>, mk_err: impl Fn(&str, &str) -> E + 'static) -> Self {
        Self {
            tokens,
            pos: 0,
            mk_err: Box::new(mk_err),
        }
    }

    pub fn cur(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    pub fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        self.pos += 1;
        t
    }

    /// Consume and return the next identifier, or return an error built by
    /// `mk_err(expected_label, found_debug)`.
    pub fn ident(&mut self, what: &str) -> Result<String, E> {
        match self.bump() {
            Token::Ident(s) => Ok(s),
            other => Err((self.mk_err)(what, &format!("{other:?}"))),
        }
    }

    /// Consume and return the next non-negative integer, or error.
    pub fn uint(&mut self, what: &str) -> Result<u64, E> {
        match self.bump() {
            Token::Int(n) if n >= 0 => Ok(n as u64),
            other => Err((self.mk_err)(what, &format!("{other:?}"))),
        }
    }

    /// Consume the next token and require it to equal `want`, or error.
    pub fn expect(&mut self, want: Token, label: &str) -> Result<(), E> {
        let got = self.bump();
        if got == want {
            Ok(())
        } else {
            Err((self.mk_err)(label, &format!("{got:?}")))
        }
    }

    /// Parse a tensor type `dtype[n]` (no trailing semicolon).
    pub fn ty(&mut self, bad_type: impl Fn(String) -> E) -> Result<TensorType, E> {
        let name = self.ident("dtype")?;
        let dtype = DType::parse(&name).ok_or_else(|| bad_type(name))?;
        self.expect(Token::LBracket, "'['")?;
        let n = self.uint("tensor length")? as usize;
        self.expect(Token::RBracket, "']'")?;
        Ok(TensorType::new(dtype, vec![n]))
    }
}
