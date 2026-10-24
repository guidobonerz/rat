//! rat REST API: assemble, disassemble and convert over HTTP, with an
//! OpenAPI description (`/openapi.json`) and Swagger UI (`/docs`).
//!
//! Requests only see the files they upload (an in-memory file system);
//! nothing on the server's disk is read or written.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Request};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use rat_asm::{MemVfs, Options};
use rat_core::{Dialect, Severity};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const BODY_LIMIT: usize = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// A source or data file: plain text, or `{"base64": "..."}` for binaries.
#[derive(Deserialize, ToSchema, Clone)]
#[serde(untagged)]
pub enum FileContent {
    Text(String),
    Binary { base64: String },
}

#[derive(Deserialize, ToSchema)]
pub struct AssembleRequest {
    /// Files by path, e.g. `{"main.asm": "...", "gfx/logo.bin": {"base64": "..."}}`.
    pub files: BTreeMap<String, FileContent>,
    /// Path of the main source in `files`.
    pub main: String,
    /// CPU (6502, 6510, 2a03, 6507, 65816, spc700, z80, 6809, 68000, 68020).
    pub cpu: Option<String>,
    /// Platform (c64, nes, snes, amiga, cpc, ...).
    pub platform: Option<String>,
    /// Main file dialect: acme, kick, rat.
    pub syntax: Option<String>,
    /// Output format.
    pub format: Option<String>,
    /// `-D` defines.
    #[serde(default)]
    pub defines: BTreeMap<String, String>,
    /// Include the listing.
    #[serde(default)]
    pub listing: bool,
}

#[derive(Serialize, ToSchema)]
pub struct OutputFile {
    pub name: String,
    pub format: String,
    pub size: usize,
    /// File contents, base64.
    pub base64: String,
}

#[derive(Serialize, ToSchema)]
pub struct DiagnosticOut {
    /// error, warning or info.
    pub severity: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    /// rustc-style rendering with the source line.
    pub rendered: String,
}

#[derive(Serialize, ToSchema)]
pub struct SymbolOut {
    pub name: String,
    pub value: i64,
    pub label: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AssembleResponse {
    pub ok: bool,
    pub passes: u32,
    pub cpu: String,
    pub platform: String,
    pub outputs: Vec<OutputFile>,
    pub diagnostics: Vec<DiagnosticOut>,
    pub symbols: Vec<SymbolOut>,
    /// `.print` output.
    pub prints: Vec<String>,
    pub listing: Option<String>,
    /// VICE monitor labels (loadable by mimic's debugger).
    pub vice_labels: String,
}

#[derive(Deserialize, ToSchema)]
pub struct DisassembleRequest {
    pub cpu: String,
    /// Load address (default 0).
    #[serde(default)]
    pub org: u32,
    /// Bytes, base64.
    pub base64: String,
    /// 65816: 16-bit accumulator.
    #[serde(default)]
    pub a16: bool,
    /// 65816: 16-bit index registers.
    #[serde(default)]
    pub i16: bool,
}

#[derive(Serialize, ToSchema)]
pub struct DisasmLine {
    pub addr: u32,
    /// Instruction bytes, hex.
    pub bytes: String,
    pub text: String,
}

#[derive(Serialize, ToSchema)]
pub struct DisassembleResponse {
    pub lines: Vec<DisasmLine>,
    /// rat-dialect source.
    pub source: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ConvertRequest {
    pub source: String,
    /// File name (its extension helps detect the dialect).
    #[serde(default)]
    pub name: Option<String>,
    pub from: Option<String>,
    pub to: String,
}

#[derive(Serialize, ToSchema)]
pub struct ConvertResponse {
    pub source: String,
    pub warnings: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CpuOut {
    pub id: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub endian: String,
    pub addr_bits: u8,
    pub mnemonics: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DirectiveOut {
    pub name: String,
    pub args: String,
    pub description: String,
}

#[derive(Serialize, ToSchema)]
pub struct PlatformOut {
    pub id: String,
    pub aliases: Vec<String>,
    pub name: String,
    pub cpu: String,
    /// Allowed output formats; the first is the default.
    pub formats: Vec<String>,
    pub default_org: Option<u32>,
    pub directives: Vec<DirectiveOut>,
    pub functions: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct FormatOut {
    pub name: String,
    pub extension: String,
    pub description: String,
    /// Platforms that allow it (`*` = all).
    pub platforms: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorOut {
    pub error: String,
}

fn bad(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(ErrorOut { error: msg.into() })).into_response()
}

#[derive(OpenApi)]
#[openapi(
    info(title = "rat assembler API", description = "Assemble, disassemble and convert ACME / KickAssembler / rat sources for 6502, 65816, SPC700, Z80, 6809 and 68000/68020."),
    paths(assemble, disassemble, convert, cpus, platforms, formats, health),
    components(schemas(AssembleRequest, FileContent, AssembleResponse, OutputFile, DiagnosticOut, SymbolOut, DisassembleRequest, DisassembleResponse, DisasmLine, ConvertRequest, ConvertResponse, CpuOut, PlatformOut, DirectiveOut, FormatOut, ErrorOut))
)]
pub struct ApiDoc;

/// The router (also used by tests).
pub fn app() -> Router {
    Router::new()
        .route("/assemble", post(assemble))
        .route("/disassemble", post(disassemble))
        .route("/convert", post(convert))
        .route("/cpus", get(cpus))
        .route("/platforms", get(platforms))
        .route("/formats", get(formats))
        .route("/health", get(health))
        .merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

/// Run the server until interrupted.
pub fn serve(bind: &str) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(bind).await.map_err(|e| format!("cannot bind {bind}: {e}"))?;
        eprintln!("rat REST API on http://{bind}  (docs: http://{bind}/docs)");
        axum::serve(listener, app()).await.map_err(|e| e.to_string())
    })
}

fn do_assemble(req: AssembleRequest) -> Result<AssembleResponse, String> {
    let mut vfs = MemVfs::new();
    for (path, content) in &req.files {
        let data = match content {
            FileContent::Text(t) => t.clone().into_bytes(),
            FileContent::Binary { base64 } => B64.decode(base64).map_err(|e| format!("{path}: bad base64: {e}"))?,
        };
        vfs.add(path, data);
    }
    if !req.files.contains_key(&req.main) {
        return Err(format!("main file `{}` is not in `files`", req.main));
    }
    let syntax = match req.syntax.as_deref() {
        Some(s) => Some(Dialect::parse(s).ok_or_else(|| format!("unknown syntax `{s}`"))?),
        None => None,
    };
    let opts = Options {
        main: rat_asm::normalize(&req.main),
        syntax,
        cpu: req.cpu,
        platform: req.platform,
        format: req.format,
        output: None,
        defines: req.defines.into_iter().collect(),
        include_dirs: Vec::new(),
        max_passes: 20,
        listing: req.listing,
        hunk_bases: Vec::new(),
    };
    let r = rat_asm::assemble(&opts, &vfs);
    let diagnostics = r
        .diagnostics
        .iter()
        .map(|d| {
            let loc = d.span.and_then(|s| r.sources.location(s));
            DiagnosticOut {
                severity: match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "info",
                }
                .into(),
                message: d.message.clone(),
                file: loc.as_ref().map(|l| l.0.clone()),
                line: loc.as_ref().map(|l| l.1),
                column: loc.as_ref().map(|l| l.2),
                rendered: d.render(&r.sources, false),
            }
        })
        .collect();
    Ok(AssembleResponse {
        ok: r.ok,
        passes: r.passes,
        cpu: r.cpu.clone(),
        platform: r.platform.clone(),
        outputs: r.outputs.iter().map(|o| OutputFile { name: o.name.clone(), format: o.format.clone(), size: o.data.len(), base64: B64.encode(&o.data) }).collect(),
        diagnostics,
        symbols: r.symbols.iter().map(|s| SymbolOut { name: s.name.clone(), value: s.value, label: s.is_label }).collect(),
        prints: r.prints.clone(),
        listing: if opts.listing { Some(r.listing.clone()) } else { None },
        vice_labels: rat_asm::vice_labels(&r.symbols, &r.breakpoints),
    })
}

/// Multipart form: file parts (the part's file name is the path), plus
/// text fields `main`, `cpu`, `platform`, `syntax`, `format`, `listing`
/// and `define` (`NAME=VALUE`, repeatable).
async fn multipart_request(mut mp: Multipart) -> Result<AssembleRequest, String> {
    let mut req = AssembleRequest {
        files: BTreeMap::new(),
        main: String::new(),
        cpu: None,
        platform: None,
        syntax: None,
        format: None,
        defines: BTreeMap::new(),
        listing: false,
    };
    while let Some(field) = mp.next_field().await.map_err(|e| e.to_string())? {
        let name = field.name().unwrap_or_default().to_owned();
        if let Some(file) = field.file_name().map(str::to_owned) {
            let data = field.bytes().await.map_err(|e| e.to_string())?;
            req.files.insert(file, FileContent::Binary { base64: B64.encode(&data) });
            continue;
        }
        let text = field.text().await.map_err(|e| e.to_string())?;
        match name.as_str() {
            "main" => req.main = text,
            "cpu" => req.cpu = Some(text),
            "platform" => req.platform = Some(text),
            "syntax" => req.syntax = Some(text),
            "format" => req.format = Some(text),
            "listing" => req.listing = matches!(text.as_str(), "1" | "true" | "yes"),
            "define" => {
                let (k, v) = text.split_once('=').unwrap_or((&text, ""));
                req.defines.insert(k.to_owned(), v.to_owned());
            }
            _ => return Err(format!("unknown form field `{name}`")),
        }
    }
    if req.main.is_empty() {
        if let Some(first) = req.files.keys().next() {
            req.main = first.clone();
        }
    }
    Ok(req)
}

/// Assemble a project (JSON or multipart/form-data).
#[utoipa::path(post, path = "/assemble", request_body(content = AssembleRequest, description = "JSON body, or multipart/form-data with file parts and text fields main/cpu/platform/syntax/format/define/listing"),
    responses((status = 200, description = "Assembly ran (check `ok`)", body = AssembleResponse), (status = 400, body = ErrorOut), (status = 504, body = ErrorOut)))]
async fn assemble(request: Request) -> Response {
    let ct = request.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let req = if ct.starts_with("multipart/form-data") {
        match Multipart::from_request(request, &()).await {
            Ok(mp) => match multipart_request(mp).await {
                Ok(r) => r,
                Err(e) => return bad(StatusCode::BAD_REQUEST, e),
            },
            Err(e) => return bad(StatusCode::BAD_REQUEST, e.to_string()),
        }
    } else {
        match Json::<AssembleRequest>::from_request(request, &()).await {
            Ok(Json(r)) => r,
            Err(e) => return bad(StatusCode::BAD_REQUEST, e.body_text()),
        }
    };
    let job = tokio::task::spawn_blocking(move || do_assemble(req));
    match tokio::time::timeout(TIMEOUT, job).await {
        Ok(Ok(Ok(r))) => Json(r).into_response(),
        Ok(Ok(Err(e))) => bad(StatusCode::BAD_REQUEST, e),
        Ok(Err(e)) => bad(StatusCode::INTERNAL_SERVER_ERROR, format!("assembler crashed: {e}")),
        Err(_) => bad(StatusCode::GATEWAY_TIMEOUT, "assembly took too long"),
    }
}

/// Disassemble bytes (rat syntax).
#[utoipa::path(post, path = "/disassemble", request_body = DisassembleRequest, responses((status = 200, body = DisassembleResponse), (status = 400, body = ErrorOut)))]
async fn disassemble(Json(req): Json<DisassembleRequest>) -> Response {
    let bytes = match B64.decode(&req.base64) {
        Ok(b) => b,
        Err(e) => return bad(StatusCode::BAD_REQUEST, format!("bad base64: {e}")),
    };
    match rat_asm::disassemble(&req.cpu, &bytes, req.org, req.a16, req.i16) {
        Ok(lines) => {
            let mut source = format!(".cpu {}\n.org ${:x}\n", req.cpu, req.org);
            for (_, _, t) in &lines {
                source.push_str("    ");
                source.push_str(t);
                source.push('\n');
            }
            let lines = lines
                .into_iter()
                .map(|(addr, b, text)| DisasmLine { addr, bytes: b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "), text })
                .collect();
            Json(DisassembleResponse { lines, source }).into_response()
        }
        Err(e) => bad(StatusCode::BAD_REQUEST, e),
    }
}

/// Convert a source between ACME, KickAssembler and rat syntax.
#[utoipa::path(post, path = "/convert", request_body = ConvertRequest, responses((status = 200, body = ConvertResponse), (status = 400, body = ErrorOut)))]
async fn convert(Json(req): Json<ConvertRequest>) -> Response {
    let Some(to) = Dialect::parse(&req.to) else { return bad(StatusCode::BAD_REQUEST, format!("unknown dialect `{}`", req.to)) };
    let from = match req.from.as_deref() {
        Some(f) => match Dialect::parse(f) {
            Some(d) => Some(d),
            None => return bad(StatusCode::BAD_REQUEST, format!("unknown dialect `{f}`")),
        },
        None => None,
    };
    let name = req.name.unwrap_or_else(|| "source".into());
    match rat_asm::convert(&name, &req.source, from, to) {
        Ok(c) => Json(ConvertResponse { source: c.text, warnings: c.warnings }).into_response(),
        Err(e) => bad(StatusCode::BAD_REQUEST, e),
    }
}

/// Supported CPUs.
#[utoipa::path(get, path = "/cpus", responses((status = 200, body = Vec<CpuOut>)))]
async fn cpus() -> Json<Vec<CpuOut>> {
    Json(
        rat_cpu::all()
            .into_iter()
            .map(|c| {
                let i = c.info();
                CpuOut {
                    id: i.id.into(),
                    aliases: i.aliases.iter().map(|s| s.to_string()).collect(),
                    description: i.description.into(),
                    endian: i.endian.into(),
                    addr_bits: i.addr_bits,
                    mnemonics: c.mnemonics(),
                }
            })
            .collect(),
    )
}

/// Platforms with their formats, directives and functions.
#[utoipa::path(get, path = "/platforms", responses((status = 200, body = Vec<PlatformOut>)))]
async fn platforms() -> Json<Vec<PlatformOut>> {
    Json(
        rat_platform::PLATFORMS
            .iter()
            .map(|p| PlatformOut {
                id: p.id.into(),
                aliases: p.aliases.iter().map(|s| s.to_string()).collect(),
                name: p.name.into(),
                cpu: p.cpu.into(),
                formats: p.all_formats().iter().map(|f| f.name().to_owned()).collect(),
                default_org: p.default_org,
                directives: p.directives.iter().map(|d| DirectiveOut { name: d.name.into(), args: d.args.into(), description: d.description.into() }).collect(),
                functions: p.functions.iter().map(|s| s.to_string()).collect(),
            })
            .collect(),
    )
}

/// Output formats.
#[utoipa::path(get, path = "/formats", responses((status = 200, body = Vec<FormatOut>)))]
async fn formats() -> Json<Vec<FormatOut>> {
    Json(
        rat_output::Format::ALL
            .iter()
            .map(|f| FormatOut {
                name: f.name().into(),
                extension: f.extension().into(),
                description: f.description().into(),
                platforms: if rat_platform::GENERIC_FORMATS.contains(f) {
                    vec!["*".into()]
                } else {
                    rat_platform::PLATFORMS.iter().filter(|p| p.formats.contains(f)).map(|p| p.id.to_owned()).collect()
                },
            })
            .collect(),
    )
}

/// Liveness check.
#[utoipa::path(get, path = "/health", responses((status = 200, body = String)))]
async fn health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn call(req: axum::http::Request<Body>) -> (StatusCode, serde_json::Value) {
        let resp = app().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null))
    }

    fn json(path: &str, v: serde_json::Value) -> axum::http::Request<Body> {
        axum::http::Request::post(path).header("content-type", "application/json").body(Body::from(v.to_string())).unwrap()
    }

    #[tokio::test]
    async fn assemble_json() {
        let (st, v) = call(json("/assemble", serde_json::json!({
            "files": {"main.asm": "BasicUpstart2(start)\nstart: inc $d020\n jmp start\n"},
            "main": "main.asm"
        })))
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["outputs"][0]["format"], "prg");
        assert!(v["vice_labels"].as_str().unwrap().contains(".start"));
    }

    #[tokio::test]
    async fn assemble_reports_errors() {
        let (_, v) = call(json("/assemble", serde_json::json!({"files": {"a.rat": "lda #$1000\n"}, "main": "a.rat"}))).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["diagnostics"][0]["line"], 1);
    }

    #[tokio::test]
    async fn assemble_multipart() {
        let b = "XBOUNDARY";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"z.rat\"\r\n\r\n.cpu z80\n ld a,5\n ret\n\r\n--{b}\r\nContent-Disposition: form-data; name=\"main\"\r\n\r\nz.rat\r\n--{b}--\r\n"
        );
        let req = axum::http::Request::post("/assemble").header("content-type", format!("multipart/form-data; boundary={b}")).body(Body::from(body)).unwrap();
        let (st, v) = call(req).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["outputs"][0]["base64"], B64.encode([0x3e, 0x05, 0xc9]));
    }

    #[tokio::test]
    async fn disassemble_and_convert() {
        let (_, v) = call(json("/disassemble", serde_json::json!({"cpu": "6502", "org": 4096, "base64": B64.encode([0xa9, 0x01, 0x60])}))).await;
        assert_eq!(v["lines"][0]["text"], "lda #$01");
        let (_, v) = call(json("/convert", serde_json::json!({"source": "!byte 1\n", "from": "acme", "to": "kick"}))).await;
        assert!(v["source"].as_str().unwrap().contains(".byte 1"), "{v}");
    }

    #[tokio::test]
    async fn listings_and_openapi() {
        let (_, v) = call(axum::http::Request::get("/platforms").body(Body::empty()).unwrap()).await;
        assert!(v.as_array().unwrap().iter().any(|p| p["id"] == "nes"));
        let (st, v) = call(axum::http::Request::get("/openapi.json").body(Body::empty()).unwrap()).await;
        assert_eq!(st, StatusCode::OK);
        assert!(v["paths"]["/assemble"].is_object());
    }
}
