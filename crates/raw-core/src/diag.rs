//! Diagnostics with a rustc-style text renderer.

use std::fmt::Write as _;

use crate::span::{SourceMap, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, span: Option<Span>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            message: message.into(),
            span,
            notes: Vec::new(),
        }
    }

    pub fn warning(message: impl Into<String>, span: Option<Span>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Warning,
            message: message.into(),
            span,
            notes: Vec::new(),
        }
    }

    pub fn info(message: impl Into<String>, span: Option<Span>) -> Diagnostic {
        Diagnostic {
            severity: Severity::Info,
            message: message.into(),
            span,
            notes: Vec::new(),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Render like rustc: header, `--> file:line:col`, the source line
    /// and a caret underline. `color` adds ANSI colors.
    pub fn render(&self, map: &SourceMap, color: bool) -> String {
        let (label, code) = match self.severity {
            Severity::Error => ("error", "\x1b[1;31m"),
            Severity::Warning => ("warning", "\x1b[1;33m"),
            Severity::Info => ("info", "\x1b[1;36m"),
        };
        let (c, b, r) = if color {
            (code, "\x1b[1;34m", "\x1b[0m")
        } else {
            ("", "", "")
        };
        let mut out = String::new();
        let _ = writeln!(out, "{c}{label}{r}: {}", self.message);
        if let Some(span) = self.span {
            if let Some(file) = map.get(span.file) {
                let (line, col) = file.line_col(span.start);
                let text = file.line_text(line);
                let width = line.to_string().len();
                let _ = writeln!(out, "{b}{:width$}-->{r} {}:{line}:{col}", "", file.name);
                let _ = writeln!(out, "{b}{:width$} |{r}", "");
                let _ = writeln!(out, "{b}{line} |{r} {text}");
                let len = if span.end > span.start {
                    let (end_line, end_col) = file.line_col(span.end);
                    if end_line == line {
                        end_col.saturating_sub(col).max(1)
                    } else {
                        text.chars().count().saturating_sub(col - 1).max(1)
                    }
                } else {
                    1
                };
                let pad: String = text
                    .chars()
                    .take(col - 1)
                    .map(|ch| if ch == '\t' { '\t' } else { ' ' })
                    .collect();
                let _ = writeln!(out, "{b}{:width$} |{r} {pad}{c}{}{r}", "", "^".repeat(len));
            }
        }
        for note in &self.notes {
            let _ = writeln!(out, "  = note: {note}");
        }
        out
    }
}
