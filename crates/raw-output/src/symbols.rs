//! Symbol files: VICE labels (`al C:xxxx .name`, read by mimic's
//! debugger), KickAssembler `.sym`, ACME label lists.

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Symbol {
    /// Fully qualified name (`scope.name`).
    pub name: String,
    pub value: i64,
    /// Address label (as opposed to a plain constant).
    pub is_label: bool,
}

pub fn vice_labels(syms: &[Symbol], breaks: &[u32]) -> String {
    let mut s = String::new();
    for sym in syms.iter().filter(|s| s.is_label) {
        s.push_str(&format!("al C:{:04x} .{}\n", sym.value as u32 & 0xff_ffff, sym.name));
    }
    for b in breaks {
        s.push_str(&format!("break {b:04x}\n"));
    }
    s
}

/// KickAssembler `.sym`: dotted names become `.namespace` blocks so the
/// file can be imported again.
pub fn kick_sym(syms: &[Symbol]) -> String {
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Ns<'a> {
        syms: Vec<(&'a str, &'a Symbol)>,
        children: BTreeMap<&'a str, Ns<'a>>,
    }
    let mut root = Ns::default();
    for sym in syms {
        let parts: Vec<&str> = sym.name.split('.').collect();
        let mut node = &mut root;
        for p in &parts[..parts.len() - 1] {
            node = node.children.entry(p).or_default();
        }
        node.syms.push((parts[parts.len() - 1], sym));
    }
    fn write(ns: &Ns<'_>, depth: usize, out: &mut String) {
        let pad = "    ".repeat(depth);
        for (n, sym) in &ns.syms {
            let kw = if sym.is_label { ".label" } else { ".const" };
            out.push_str(&format!("{pad}{kw} {n}=${:x}\n", sym.value));
        }
        for (n, child) in &ns.children {
            out.push_str(&format!("{pad}.namespace {n} {{\n"));
            write(child, depth + 1, out);
            out.push_str(&format!("{pad}}}\n"));
        }
    }
    let mut s = String::new();
    write(&root, 0, &mut s);
    s
}

pub fn acme_labels(syms: &[Symbol]) -> String {
    let mut s = String::new();
    for sym in syms {
        s.push_str(&format!("\t{}\t= ${:x}\n", sym.name, sym.value));
    }
    s
}
