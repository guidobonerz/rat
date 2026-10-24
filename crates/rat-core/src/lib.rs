//! Foundation shared by every rat crate: source files and spans,
//! diagnostics, the token model with a configurable lexer, and the
//! statement IR that all three dialect front ends (ACME, KickAssembler,
//! rat) lower to.

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod span;
pub mod token;

pub use ast::*;
pub use diag::{Diagnostic, Severity};
pub use span::{FileId, SourceFile, SourceMap, Span};
pub use token::{Tok, Token};

/// Source dialect of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    Acme,
    Kick,
    Rat,
}

impl Dialect {
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Acme => "acme",
            Dialect::Kick => "kick",
            Dialect::Rat => "rat",
        }
    }

    pub fn parse(s: &str) -> Option<Dialect> {
        match s.to_ascii_lowercase().as_str() {
            "acme" => Some(Dialect::Acme),
            "kick" | "kickass" | "kickassembler" => Some(Dialect::Kick),
            "rat" => Some(Dialect::Rat),
            _ => None,
        }
    }

    /// Dialect implied by a file extension (`.a` ACME, `.asm` Kick, `.rat` rat).
    pub fn from_extension(path: &str) -> Option<Dialect> {
        let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "a" | "acme" | "a65" => Some(Dialect::Acme),
            "asm" | "kick" | "kickass" | "inc" => Some(Dialect::Kick),
            "rat" => Some(Dialect::Rat),
            _ => None,
        }
    }

    pub const ALL: [Dialect; 3] = [Dialect::Acme, Dialect::Kick, Dialect::Rat];
}

/// Byte order of multi-byte data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endian {
    Little,
    Big,
}
