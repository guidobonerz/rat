//! `rat`: multi-CPU, multi-dialect assembler. Plain CLI and REST server.

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rat_asm::{FsVfs, Options};
use rat_core::Dialect;

#[derive(Parser)]
#[command(name = "rat", version, about = "rat assembles words: ACME, KickAssembler and rat syntax for 6502/6510/2A03/6507, 65816, SPC700, Z80, 6809, 68000/68020")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Assemble a source file.
    Build {
        /// Main source file.
        source: String,
        /// Output file (default: source name with the format's extension).
        #[arg(short, long)]
        output: Option<String>,
        /// CPU (overrides the platform default).
        #[arg(long)]
        cpu: Option<String>,
        /// Target platform (c64, nes, snes, amiga, ...).
        #[arg(short, long)]
        platform: Option<String>,
        /// Source dialect of the main file: acme, kick, rat.
        #[arg(short, long)]
        syntax: Option<String>,
        /// Output format (prg, bin, xex, nes, sfc, a26, p, amsdos, hunk, crt, d64, hex, srec).
        #[arg(short, long)]
        format: Option<String>,
        /// Define a symbol: NAME or NAME=VALUE (also in cmdLineVars).
        #[arg(short = 'D', long = "define")]
        defines: Vec<String>,
        /// Include directory.
        #[arg(short = 'I', long = "include")]
        include: Vec<String>,
        /// Write a listing.
        #[arg(long)]
        listing: Option<String>,
        /// Write VICE labels (readable by mimic's debugger).
        #[arg(long = "vice-labels")]
        vice_labels: Option<String>,
        /// Write a KickAssembler .sym file.
        #[arg(long)]
        sym: Option<String>,
        /// Write an ACME label list.
        #[arg(long)]
        labels: Option<String>,
        /// Maximum passes.
        #[arg(long, default_value_t = 20)]
        passes: u32,
        /// Print a summary of outputs and symbols as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Disassemble a binary.
    Disasm {
        file: String,
        #[arg(long)]
        cpu: String,
        /// Load address (default: from a PRG header, else 0).
        #[arg(long)]
        org: Option<String>,
        /// Skip bytes at the start.
        #[arg(long, default_value_t = 0)]
        skip: usize,
        /// Treat the file as a PRG (2-byte load address).
        #[arg(long)]
        prg: bool,
        /// 65816: 16-bit accumulator / index registers.
        #[arg(long)]
        a16: bool,
        #[arg(long)]
        i16: bool,
    },
    /// Convert a source file to another dialect.
    Convert {
        file: String,
        /// Target dialect: acme, kick, rat.
        #[arg(long)]
        to: String,
        /// Source dialect (default: detected).
        #[arg(long)]
        from: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Run the REST API server.
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
    },
    /// List CPUs.
    Cpus,
    /// List platforms with formats and directives.
    Platforms,
    /// List output formats.
    Formats,
}

fn parse_num(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('$').or_else(|| s.strip_prefix("0x")) {
        u32::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

fn write(path: &str, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("cannot write {path}: {e}"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let color = std::io::stderr().is_terminal();
    match cli.cmd {
        Cmd::Build { source, output, cpu, platform, syntax, format, defines, include, listing, vice_labels, sym, labels, passes, json } => {
            let syntax = match syntax.as_deref().map(Dialect::parse) {
                Some(None) => {
                    eprintln!("error: unknown syntax (use acme, kick or rat)");
                    return ExitCode::from(2);
                }
                Some(d) => d,
                None => None,
            };
            let defines = defines
                .iter()
                .map(|d| match d.split_once('=') {
                    Some((k, v)) => (k.to_owned(), v.to_owned()),
                    None => (d.clone(), String::new()),
                })
                .collect();
            // outputs go next to the source unless a path is given
            let src_dir = Path::new(&source).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let opts = Options {
                main: source.clone(),
                syntax,
                cpu,
                platform,
                format,
                output: output.clone(),
                defines,
                include_dirs: include,
                max_passes: passes,
                listing: listing.is_some(),
                hunk_bases: Vec::new(),
            };
            let r = rat_asm::assemble(&opts, &FsVfs);
            for p in &r.prints {
                println!("{p}");
            }
            eprint!("{}", r.render_diagnostics(color));
            if !r.ok {
                eprintln!("error: assembly failed with {} error(s)", r.errors());
                return ExitCode::FAILURE;
            }
            let mut written = Vec::new();
            for o in &r.outputs {
                let path = if output.is_some() && Some(&o.name) == output.as_ref() || Path::new(&o.name).is_absolute() || src_dir.is_empty() {
                    o.name.clone()
                } else {
                    Path::new(&src_dir).join(&o.name).to_string_lossy().into_owned()
                };
                if let Err(e) = write(&path, &o.data) {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
                written.push((path, o.format.clone(), o.data.len()));
            }
            if let Some(l) = listing {
                if let Err(e) = write(&l, r.listing.as_bytes()) {
                    eprintln!("error: {e}");
                }
            }
            if let Some(v) = vice_labels {
                let _ = write(&v, rat_asm::vice_labels(&r.symbols, &r.breakpoints).as_bytes());
            }
            if let Some(s) = sym {
                let _ = write(&s, rat_asm::kick_sym(&r.symbols).as_bytes());
            }
            if let Some(s) = labels {
                let _ = write(&s, rat_asm::acme_labels(&r.symbols).as_bytes());
            }
            if json {
                let v = serde_json::json!({
                    "ok": r.ok,
                    "passes": r.passes,
                    "cpu": r.cpu,
                    "platform": r.platform,
                    "outputs": written.iter().map(|(p, f, n)| serde_json::json!({"file": p, "format": f, "size": n})).collect::<Vec<_>>(),
                    "symbols": r.symbols,
                });
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                for (p, f, n) in &written {
                    eprintln!("wrote {p} ({f}, {n} bytes) in {} pass(es)", r.passes);
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Disasm { file, cpu, org, skip, prg, a16, i16 } => {
            let data = match std::fs::read(&file) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: cannot read {file}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let is_prg = prg || file.to_ascii_lowercase().ends_with(".prg");
            let (mut base, body) = if is_prg && data.len() >= 2 {
                (u32::from(u16::from_le_bytes([data[0], data[1]])), &data[2..])
            } else {
                (0, &data[..])
            };
            if let Some(o) = org {
                match parse_num(&o) {
                    Some(v) => base = v,
                    None => {
                        eprintln!("error: bad --org");
                        return ExitCode::from(2);
                    }
                }
            }
            let body = body.get(skip..).unwrap_or(&[]);
            match rat_asm::disassemble(&cpu, body, base + skip as u32, a16, i16) {
                Ok(lines) => {
                    println!(".cpu {cpu}");
                    println!(".org ${:04x}", base + skip as u32);
                    for (addr, bytes, text) in lines {
                        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
                        println!("    {text:<28} ; {addr:06x}  {}", hex.join(" "));
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Cmd::Convert { file, to, from, output } => {
            let Some(to) = Dialect::parse(&to) else {
                eprintln!("error: unknown target dialect `{to}` (acme, kick, rat)");
                return ExitCode::from(2);
            };
            let from = from.as_deref().and_then(Dialect::parse);
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error: cannot read {file}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match rat_asm::convert(&file, &text, from, to) {
                Ok(c) => {
                    for w in &c.warnings {
                        eprintln!("warning: {w}");
                    }
                    match output {
                        Some(o) => {
                            if let Err(e) = write(&o, c.text.as_bytes()) {
                                eprintln!("error: {e}");
                                return ExitCode::FAILURE;
                            }
                        }
                        None => print!("{}", c.text),
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprint!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Cmd::Serve { bind } => match rat_server::serve(&bind) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::Cpus => {
            for c in rat_cpu::all() {
                let i = c.info();
                println!("{:<8} {:<7} {:>2}-bit  {}{}", i.id, i.endian, i.addr_bits, i.description, if i.aliases.is_empty() { String::new() } else { format!(" (aliases: {})", i.aliases.join(", ")) });
            }
            ExitCode::SUCCESS
        }
        Cmd::Platforms => {
            for p in rat_platform::PLATFORMS {
                let formats: Vec<&str> = p.all_formats().iter().map(|f| f.name()).collect();
                println!("{:<10} {:<34} cpu {:<6} formats {}", p.id, p.name, p.cpu, formats.join(", "));
                for d in p.directives {
                    println!("             .{} {}  - {}", d.name, d.args, d.description);
                }
                if !p.functions.is_empty() {
                    println!("             functions: {}", p.functions.join(", "));
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Formats => {
            for f in rat_output::Format::ALL {
                let on: Vec<&str> = if rat_platform::GENERIC_FORMATS.contains(&f) {
                    vec!["all platforms"]
                } else {
                    rat_platform::PLATFORMS.iter().filter(|p| p.formats.contains(&f)).map(|p| p.id).collect()
                };
                println!("{:<7} .{:<4} {}  [{}]", f.name(), f.extension(), f.description(), on.join(", "));
            }
            ExitCode::SUCCESS
        }
    }
}
