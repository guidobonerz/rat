//! Script values (a superset of KickAssembler's: numbers, strings,
//! booleans, lists, hashtables, structs, functions, vectors, matrices,
//! SID files, pictures, binaries, pseudocommand arguments).

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use indexmap::IndexMap;
use raw_core::MacroDef;
use raw_platform::cbm::{Picture, Sid};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HKey {
    Str(String),
}

#[derive(Debug, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<String>,
}

#[derive(Debug)]
pub struct StructVal {
    pub def: Rc<StructDef>,
    pub values: RefCell<Vec<Value>>,
}

#[derive(Debug)]
pub struct CmdArg {
    pub kind: i64,
    pub value: Value,
}

#[derive(Debug)]
pub struct BinFile {
    pub data: Vec<u8>,
    /// Template sections: name -> (offset, size).
    pub sections: Vec<(String, usize, usize)>,
}

#[derive(Debug)]
pub struct PictureVal {
    pub pic: Picture,
    pub table: RefCell<Vec<u32>>,
}

#[derive(Clone, Debug, Default)]
pub enum Value {
    /// Not resolved yet in this pass (forward reference).
    #[default]
    Undef,
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    List(Rc<RefCell<Vec<Value>>>),
    Hash(Rc<RefCell<IndexMap<HKey, Value>>>),
    StructDef(Rc<StructDef>),
    Struct(Rc<StructVal>),
    Macro(Rc<MacroDef>),
    Scope(usize),
    CmdArg(Rc<CmdArg>),
    Sid(Rc<Sid>),
    Picture(Rc<PictureVal>),
    Bin(Rc<BinFile>),
    Vector([f64; 3]),
    Matrix(Rc<RefCell<[[f64; 4]; 4]>>),
}

impl PartialEq for Value {
    fn eq(&self, o: &Value) -> bool {
        use Value::*;
        match (self, o) {
            (Undef, Undef) | (Null, Null) => true,
            (Int(a), Int(b)) => a == b,
            (Float(a), Float(b)) => a == b,
            (Int(a), Float(b)) | (Float(b), Int(a)) => (*a as f64) == *b,
            (Bool(a), Bool(b)) => a == b,
            (Str(a), Str(b)) => a == b,
            (List(a), List(b)) => Rc::ptr_eq(a, b) || *a.borrow() == *b.borrow(),
            (Hash(a), Hash(b)) => Rc::ptr_eq(a, b) || *a.borrow() == *b.borrow(),
            (Struct(a), Struct(b)) => Rc::ptr_eq(a, b) || (a.def == b.def && *a.values.borrow() == *b.values.borrow()),
            (StructDef(a), StructDef(b)) => Rc::ptr_eq(a, b) || a == b,
            (Macro(a), Macro(b)) => Rc::ptr_eq(a, b) || a == b,
            (Scope(a), Scope(b)) => a == b,
            (CmdArg(a), CmdArg(b)) => a.kind == b.kind && a.value == b.value,
            (Sid(a), Sid(b)) => a == b,
            (Picture(a), Picture(b)) => Rc::ptr_eq(a, b),
            (Bin(a), Bin(b)) => Rc::ptr_eq(a, b) || a.data == b.data,
            (Vector(a), Vector(b)) => a == b,
            (Matrix(a), Matrix(b)) => *a.borrow() == *b.borrow(),
            _ => false,
        }
    }
}

impl Value {
    pub fn str(s: impl Into<Rc<str>>) -> Value {
        Value::Str(s.into())
    }

    pub fn list(v: Vec<Value>) -> Value {
        Value::List(Rc::new(RefCell::new(v)))
    }

    pub fn is_undef(&self) -> bool {
        matches!(self, Value::Undef)
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undef => "unresolved value",
            Value::Null => "null",
            Value::Int(_) | Value::Float(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Str(_) => "string",
            Value::List(_) => "list",
            Value::Hash(_) => "hashtable",
            Value::StructDef(_) => "struct definition",
            Value::Struct(_) => "struct",
            Value::Macro(m) => match m.kind {
                raw_core::MacroKind::Function => "function",
                raw_core::MacroKind::Pseudo => "pseudocommand",
                raw_core::MacroKind::Macro => "macro",
            },
            Value::Scope(_) => "scope",
            Value::CmdArg(_) => "command argument",
            Value::Sid(_) => "SID file",
            Value::Picture(_) => "picture",
            Value::Bin(_) => "binary file",
            Value::Vector(_) => "vector",
            Value::Matrix(_) => "matrix",
        }
    }

    /// Numeric value; `None` when not a number (or unresolved).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(v) => Some(*v as f64),
            Value::Float(v) => Some(*v),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }

    /// Integer value (floats truncate).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Float(v) => Some(v.trunc() as i64),
            Value::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn truthy(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(v) => Some(*v != 0),
            Value::Float(v) => Some(*v != 0.0),
            Value::Null => Some(false),
            Value::Str(s) => Some(!s.is_empty()),
            Value::Undef => None,
            _ => Some(true),
        }
    }

    /// Hashtable key: KickAssembler compares keys by their string form.
    pub fn hkey(&self) -> HKey {
        match self {
            Value::Float(f) if f.fract() == 0.0 => HKey::Str((*f as i64).to_string()),
            other => HKey::Str(other.to_string()),
        }
    }

    /// Normalize floats that hold integers.
    pub fn num(f: f64) -> Value {
        if f.fract() == 0.0 && f.abs() < 9.0e15 {
            Value::Int(f as i64)
        } else {
            Value::Float(f)
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Undef => f.write_str("<unresolved>"),
            Value::Null => f.write_str("null"),
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => {
                if v.fract() == 0.0 && v.abs() < 1e15 {
                    write!(f, "{}.0", *v as i64)
                } else {
                    write!(f, "{v}")
                }
            }
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => f.write_str(s),
            Value::List(l) => {
                f.write_str("[")?;
                for (i, v) in l.borrow().iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Value::Hash(h) => {
                f.write_str("{")?;
                for (i, (k, v)) in h.borrow().iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    match k {
                        HKey::Str(s) => write!(f, "{s}={v}")?,
                    }
                }
                f.write_str("}")
            }
            Value::StructDef(d) => write!(f, "struct {}", d.name),
            Value::Struct(s) => {
                write!(f, "{}(", s.def.name)?;
                for (i, v) in s.values.borrow().iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str(")")
            }
            Value::Macro(m) => write!(f, "{} {}", self.type_name(), m.name),
            Value::Scope(_) => f.write_str("<scope>"),
            Value::CmdArg(c) => write!(f, "CmdArgument({},{})", c.kind, c.value),
            Value::Sid(s) => write!(f, "SID({})", s.name),
            Value::Picture(p) => write!(f, "Picture({}x{})", p.pic.width, p.pic.height),
            Value::Bin(b) => write!(f, "Binary({} bytes)", b.data.len()),
            Value::Vector(v) => write!(f, "({},{},{})", v[0], v[1], v[2]),
            Value::Matrix(_) => f.write_str("<matrix>"),
        }
    }
}
