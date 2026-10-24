//! Statement IR shared by every dialect front end.
//!
//! Parsers lower ACME `!` pseudo-ops, KickAssembler `.` directives and rat
//! directives into these statements; the assembler executes them once per
//! pass. Instruction operands stay tokens: the active CPU backend parses
//! its native addressing syntax at assembly time, so a `.cpu` switch in the
//! middle of a file needs no re-parse.

use std::rc::Rc;

use crate::span::Span;
use crate::token::Token;
use crate::{Dialect, Endian};

/// Kind of a symbol name, for ACME's scoping rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameKind {
    /// Ordinary name, resolved through the scope chain.
    Plain,
    /// ACME `.local`: local to the current zone.
    ZoneLocal,
    /// ACME `@cheap` / rat `@local`: local to the region since the last
    /// global label.
    Cheap,
    /// KickAssembler `@name`: the root scope, escaping the current one.
    Root,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Name {
    pub kind: NameKind,
    pub text: String,
}

impl Name {
    pub fn plain(s: impl Into<String>) -> Name {
        Name {
            kind: NameKind::Plain,
            text: s.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Pos,
    /// Logical not (Kick `!`).
    Not,
    /// Bitwise not (ACME `!`, Kick `~`).
    BitNot,
    /// Low byte `<`.
    Lo,
    /// High byte `>`.
    Hi,
    /// Bank byte (ACME `^`).
    Bank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// Integer division when both sides are integers (ACME `/`).
    Div,
    /// Always floating point (KickAssembler `/`).
    FDiv,
    /// Integer division (ACME `DIV`).
    IntDiv,
    Mod,
    Pow,
    Shl,
    Shr,
    /// Logical shift right (ACME `>>>`, `LSR`).
    Lsr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LAnd,
    LOr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Char(char),
    Bool(bool),
    Null,
    /// Program counter (`*`, `$`).
    Pc,
    Ident(Name),
    /// Anonymous label reference: `key` names the label family (`-`, `--`,
    /// `+`, `!`, `!name`); `offset` < 0 counts backwards, > 0 forwards.
    Anon { key: String, offset: i32 },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Field(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    List(Vec<Expr>),
    Assign(AssignOp, Box<Expr>, Box<Expr>),
    IncDec { target: Box<Expr>, inc: bool, post: bool },
    /// `defined(name)` / ACME `!ifdef` style test.
    Defined(Name),
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }

    pub fn int(v: i64, span: Span) -> Expr {
        Expr::new(ExprKind::Int(v), span)
    }

    pub fn string(s: impl Into<String>, span: Span) -> Expr {
        Expr::new(ExprKind::Str(s.into()), span)
    }

    pub fn ident(s: impl Into<String>, span: Span) -> Expr {
        Expr::new(ExprKind::Ident(Name::plain(s)), span)
    }

    /// The identifier text when this is a plain name.
    pub fn as_ident(&self) -> Option<&str> {
        match &self.kind {
            ExprKind::Ident(n) if n.kind == NameKind::Plain => Some(&n.text),
            _ => None,
        }
    }
}

pub type Block = Vec<Stmt>;

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

impl Stmt {
    pub fn new(kind: StmtKind, span: Span) -> Stmt {
        Stmt { kind, span }
    }
}

/// `key=value` or positional directive argument.
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub key: Option<String>,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignKind {
    /// Immutable symbol (`.const`, `x = 1` in rat/Kick label sense).
    Const,
    /// Mutable script variable (`.var`).
    Var,
    /// Address label with explicit value (`.label x = $d020`, ACME `x = 1`).
    Label,
    /// Redefinable (ACME `!set`).
    Set,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextConv {
    /// Current encoding / conversion table.
    Current,
    Raw,
    Petscii,
    Screen,
    Named(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlignKind {
    /// Pad until `(pc & and) == equal` (ACME `!align`).
    Mask { and: Expr, equal: Expr },
    /// Pad to the next multiple (Kick `.align`, rat `.align`).
    Multiple(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryKind {
    Raw,
    /// Skip the two-byte load address (Kick `.import c64`).
    C64,
    /// Bytes of a text file, as text.
    Text,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgLevel {
    Warn,
    Error,
    /// Fatal, stops at once (ACME `!serious`).
    Serious,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroKind {
    Macro,
    /// Kick `.pseudocommand`: arguments are addressing-mode operands.
    Pseudo,
    Function,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    /// ACME `~name`: by reference.
    pub by_ref: bool,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroDef {
    pub name: String,
    pub params: Vec<Param>,
    /// Last parameter collects the rest (rat `...`).
    pub variadic: bool,
    pub body: Block,
    pub kind: MacroKind,
    pub dialect: Dialect,
    pub span: Span,
    /// Function body that is a single expression (`.fn f(x) = x*2`).
    pub expr_body: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instr {
    /// Label in front of a pseudocommand / macro call (`lbl: mov #1 : x`).
    pub label: Option<Name>,
    pub mnemonic: String,
    /// Mnemonic suffix: `lda.z`, `move.l`, ACME `lda+2`.
    pub suffix: Option<String>,
    pub operand: Vec<Token>,
    pub dialect: Dialect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    /// Define `name` at the current PC. `multi` is Kick's `!name:`.
    Label(Name),
    /// Anonymous label of a family (`-`, `+`, `!`, `!name`).
    AnonLabel(String),
    Assign { name: Name, value: Expr, kind: AssignKind },
    Eval(Expr),
    Instr(Instr),
    Data { size: u8, endian: Option<Endian>, items: Vec<Expr> },
    Text { conv: TextConv, xor: Option<Expr>, items: Vec<Expr> },
    Hex(Vec<u8>),
    /// `count` items of `size` bytes; with `index_var` the value is
    /// evaluated per item with that variable bound to the index.
    Fill { count: Expr, value: Option<Expr>, size: u8, index_var: Option<String> },
    /// Kick `.lohifill`: `name.lo` and `name.hi` tables.
    LoHiFill { name: Option<Name>, count: Expr, value: Expr, index_var: String },
    Skip(Expr),
    Align { kind: AlignKind, fill: Option<Expr> },
    /// Set PC. `name` names the memory block (Kick). `overlay`: ACME
    /// `*= addr, overlay`. `invisible`: no output (ACME).
    Org { addr: Expr, name: Option<Expr>, overlay: bool, invisible: bool },
    PseudoPc { addr: Expr, body: Option<Block> },
    RealPc,
    Include { path: Expr, once: bool },
    ImportOnce,
    Binary { path: Expr, size: Option<Expr>, offset: Option<Expr>, kind: BinaryKind },
    If { cond: Expr, then: Block, else_: Option<Block> },
    IfDef { name: Name, negate: bool, then: Block, else_: Option<Block> },
    ForC { init: Vec<Stmt>, cond: Option<Expr>, step: Vec<Expr>, body: Block },
    /// `for var = from .. to` (inclusive when `inclusive`).
    ForRange { var: String, from: Expr, to: Expr, step: Option<Expr>, inclusive: bool, body: Block },
    ForIn { var: String, list: Expr, body: Block },
    While { cond: Expr, body: Block },
    /// ACME `!do [while|until c] { } [while|until c]`; bool = until.
    Do { pre: Option<(bool, Expr)>, body: Block, post: Option<(bool, Expr)> },
    Scope { name: Option<Name>, body: Block },
    Zone { name: Option<String>, body: Option<Block> },
    Namespace { name: String, body: Option<Block>, file: bool },
    MacroDef(Rc<MacroDef>),
    /// Macro (or function as a statement) call. `label` names the scope of
    /// the expansion (Kick `lbl: macro()`).
    Call { name: Expr, args: Vec<Expr>, label: Option<Name> },
    PseudoCall { name: String, args: Vec<Vec<Token>>, suffix: Option<String>, dialect: Dialect },
    Return(Option<Expr>),
    Print { expr: Expr, now: bool },
    Message { level: MsgLevel, args: Vec<Expr> },
    ErrorIf { cond: Expr, msg: Expr },
    /// Kick `.assert desc, actual, expected`; rat `.assert cond [, msg]`.
    Assert { desc: Option<Expr>, actual: Expr, expected: Option<Expr> },
    AssertError { desc: Expr, body: Block },
    /// Kick `.assert "desc", { code }, { code }`: both assemble to the same bytes.
    AssertCode { desc: Expr, actual: Block, expected: Block },
    Cpu(String),
    Platform { name: String, body: Option<Block> },
    /// Platform directive (`.ines`, `.xex.run`, `.snes.header` ...).
    /// `ns` is set for the namespaced form `.c64.basicupstart`.
    PlatformDir { ns: Option<String>, name: String, args: Vec<Arg>, body: Option<Block> },
    SegmentDef { name: String, params: Vec<Arg> },
    Segment { name: String, params: Vec<Arg>, body: Option<Block> },
    SegmentOut { params: Vec<Arg> },
    File { params: Vec<Arg>, body: Option<Block> },
    Disk { params: Vec<Arg>, files: Vec<Vec<Arg>> },
    MemBlock(Expr),
    Output { file: Expr, format: Option<String> },
    Encoding(Expr),
    ConvTabFile(Expr),
    Xor { value: Expr, body: Option<Block> },
    InitMem(Expr),
    /// CPU state switch: `a8`, `a16`, `i8`, `i16` (65816).
    CpuFlag(String),
    SetDp(Expr),
    Endian(Option<Endian>),
    Struct { name: String, fields: Vec<String> },
    Enum { items: Vec<(String, Option<Expr>)> },
    PpDefine(String),
    PpUndef(String),
    PpIf { branches: Vec<(Expr, Block)>, else_: Option<Block> },
    Break(Option<Expr>),
    Watch { args: Vec<Expr> },
    SymbolList(Expr),
    /// Stop reading this file.
    Eof,
    /// Kick `.zp { }`: labels inside are zero page.
    ZpBlock(Block),
    /// A group of statements without its own scope.
    Group(Block),
    /// Syntax switch, consumed by the parser.
    Syntax(String),
    Nop,
}
