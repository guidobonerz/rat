# rat

rat assembles text. It is a multi-CPU, multi-dialect assembler written in
Rust, built to produce binaries for every CPU that the
[mimic](../mimic) emulator runs. You can use it as a plain CLI or as a REST
API.

| CPU | Variants | Byte order | Used by |
|---|---|---|---|
| MOS 6502 family | `6502` (with undocumented opcodes), `6502ni`, `6510`, `2a03`, `6507` | little | C64, C128, VIC-20, PET, C16/Plus/4, Atari 8-bit, 2600, NES, Apple II |
| CMOS 65C02 | `65c02` (no Rockwell bit instructions, like mimic) | little | Apple IIe enhanced |
| WDC 65C816 | `65816` | little | SNES |
| Sony SPC700 | `spc700` | little | SNES audio |
| Zilog Z80 (incl. undocumented) | `z80` | little | Amstrad CPC, ZX81, ZX Spectrum, ColecoVision, C128 |
| Motorola 6809 | `6809` | big | Vectrex |
| Motorola 68000 / 68020 | `68000`, `68020` | big | Amiga 500…1200, Macintosh |

The encoding tables are ported from mimic's cores. The tests check every
opcode against mimic's disassemblers (about 68,000 instructions; see
[Testing](#testing)).

## Installation

Prebuilt binaries for Linux, macOS and Windows are attached to each
[GitHub release](https://github.com/guidobonerz/rat/releases). Download the
archive for your system and unpack it.

### Linux

```
tar -xzf rat-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd rat-v0.1.0-x86_64-unknown-linux-gnu
chmod +x rat                      # only needed if the executable bit got lost
sudo mv rat /usr/local/bin/       # or ~/.local/bin, if that is on your PATH
rat --help
```

Use the `aarch64-unknown-linux-gnu` archive on ARM64 machines such as a
Raspberry Pi 4/5 with a 64-bit OS.

### macOS

Pick `aarch64-apple-darwin` for Apple Silicon (M1 and later) and
`x86_64-apple-darwin` for Intel Macs.

```
tar -xzf rat-v0.1.0-aarch64-apple-darwin.tar.gz
cd rat-v0.1.0-aarch64-apple-darwin
chmod +x rat
xattr -d com.apple.quarantine rat # the binary is not signed or notarized
sudo mv rat /usr/local/bin/
rat --help
```

Without the `xattr` step, macOS blocks the binary with a message that it
"cannot be opened because the developer cannot be verified". As an
alternative, try to run it once, then allow it under
*System Settings → Privacy & Security → Open Anyway*.

### Windows

Unpack the zip, for example to `C:\Tools\rat`. Then add that folder to your
`PATH` so `rat` works in any terminal:

```
setx PATH "%PATH%;C:\Tools\rat"
```

Open a new terminal and run `rat --help`. If SmartScreen warns about an
unrecognized app, click *More info → Run anyway*. If Windows flags the
downloaded file as blocked, run this in PowerShell:

```
Unblock-File C:\Tools\rat\rat.exe
```

The Linux and Windows binaries are compressed with [UPX](https://upx.github.io/).
Some antivirus tools flag UPX-packed files as suspicious. If yours quarantines
`rat.exe`, check it against `SHA256SUMS.txt` and add an exclusion, or build
from source.

### Verifying downloads

Each release contains `SHA256SUMS.txt`:

```
sha256sum -c SHA256SUMS.txt --ignore-missing          # Linux
shasum -a 256 -c SHA256SUMS.txt --ignore-missing      # macOS
Get-FileHash rat-v0.1.0-x86_64-pc-windows-msvc.zip    # Windows, compare by hand
```

### Building from source

```
cargo install --path crates/rat-cli
```

This puts `rat` in `~/.cargo/bin`. The workspace expects the
[mimic](https://github.com/guidobonerz/MIMIC) repository checked out next to
this one, in a folder named `mimic`.

## Source dialects

rat reads three dialects and can convert between them:

* **ACME**: `!` pseudo-ops, labels without colons, `.local` and `@cheap`
  labels, anonymous `-`/`+` labels, `+macro` calls, `!zone`, `!for`,
  `!if`, `!do`, `!pseudopc`, `!convtab`, `!binary`, `!source`, `!to`, …
  For 68000 sources also the Motorola data directives without `!`: `dc.b`/`dc.w`/`dc.l`
  (big endian), `ds.b`/`ds.w`/`ds.l count[, fill]` and `even`.
* **KickAssembler**: `.` directives, `label:`, `!multi:` labels, `{ }`
  scopes, and the whole script language: `.var`/`.const`/`.eval`,
  functions, macros, pseudocommands, lists, hashtables, structs,
  `.enum`, the math library, vectors and matrices, `#import`/`#if`
  preprocessor, `.namespace`, segments, `.file`, `.disk`,
  `LoadBinary`/`LoadSid`/`LoadPicture`, `.assert`. `.plugin` (Java) is not
  supported.
* **rat**: rat's own dialect, documented in [docs/rat-syntax.md](docs/rat-syntax.md).
  It is CPU-neutral, uses Kick-style directives, accepts `;` and `//`
  comments, and adds `.org`, `.include`, `.incbin`, `.fn`,
  `.for i in a..b`, `.elif`, `dc.b`/`ds.w`, `@local` and `.local` labels.

**Which dialect a file uses**, first match wins:
1. A `.syntax acme|kick|rat` (or `!syntax`) line at the top of the file.
2. `--syntax` on the command line (main file only).
3. The extension: `.a` is ACME, `.asm` is KickAssembler, `.rat` is rat.
4. The dialect of the file that includes it.

Every included file is detected on its own, so one project can mix
dialects.

Instruction operands always use the CPU's native syntax (Zilog, Motorola,
Sony, MOS), whatever the dialect.

## Platforms

A **platform** chooses the default CPU, the default origin, the allowed
output formats, the text encoding and the platform directives:

```
.platform c64        ; Kick and rat
!platform nes        ; ACME
```

Features that belong to one platform only exist on that platform.
`BasicUpstart`, `LoadSid`, `LoadPicture`, `.disk`, and the `prg`, `d64` and
`crt` formats are Commodore features. Using them on `nes`, `cpc` or `amiga`
is an error that names the platform you need. KickAssembler and ACME
sources default to `c64`, so existing sources build unchanged; rat
sources start on `generic`.

| Platform | CPU | Formats (first = default) | Platform directives |
|---|---|---|---|
| `c64` | 6510 | prg, d64, crt | `basicupstart`, `crt`, `disk`; `LoadSid()`, `LoadPicture()` |
| `c128`, `vic20`, `pet`, `plus4` | 6502 | prg, d64 | `basicupstart`, `disk`; `LoadSid()`, `LoadPicture()` |
| `atari8` | 6502 | xex | `xex.run`, `xex.init` |
| `atari2600` | 6507 | a26 | `a26.bank n` |
| `nes` | 2a03 | nes | `ines mapper= prg= chr= mirroring=`, `nes.chr { }`, `nes.bank n` |
| `snes` | 65816 | sfc | `snes.header title= map=lorom\|hirom\|exhirom`, `snes.spc [addr] { }` |
| `cpc` | z80 | amsdos | `amsdos name= exec=` |
| `zx81` | z80 | p | |
| `spectrum` | z80 | tap, sna | `tap name= exec= loader=` (BASIC loader `CLEAR/LOAD ""CODE/RANDOMIZE USR`), `sna exec= sp= border= im= ei=` |
| `apple2`, `apple2e` | 6502, 65c02 | dsk (DOS 3.3 image with the program as a binary file; boot a DOS and `BRUN` it) | `dsk name= volume= type= locked=` |
| `mac` | 68000 | bin | |
| `coleco` | z80 | bin | `coleco.header start` |
| `vectrex` | 6809 | bin | `vectrex.header "TITLE"` |
| `amiga`, `amiga1200` | 68000, 68020 | hunk (with RELOC32) | `amiga.section name, code\|data\|bss` |
| `generic`, `sim6502` | 6502 | bin | |

`bin`, `hex` (Intel HEX) and `srec` (Motorola S-records) are available on
every platform. Run `rat platforms` and `rat formats` for the full lists.

## CLI

```
rat build main.asm                       # dialect from extension, c64 → main.prg
rat build game.rat -p nes -o game.nes
rat build demo.a -D DEBUG=1 -I lib --listing demo.lst --vice-labels demo.vs
rat build intro.asm -f d64               # D64 with one PRG
rat disasm game.prg --cpu 6510           # PRG load address is honoured
rat disasm rom.bin --cpu 6809 --org $e000
rat convert old.a --to kick -o new.asm   # ACME → KickAssembler
rat serve --bind 127.0.0.1:8080          # REST API
rat cpus | rat platforms | rat formats
```

Errors are printed rustc-style with the source line.

* `--vice-labels` writes `al C:xxxx .name` lines that mimic's debugger (and
  VICE) load directly.
* `--sym` writes a KickAssembler `.sym` file.
* `--labels` writes an ACME label list.

## REST API

`rat serve` starts an axum server. The API is described by OpenAPI at
`/openapi.json`, with Swagger UI at `/docs`.

| Method | Path | |
|---|---|---|
| POST | `/assemble` | JSON `{files: {path: text \| {base64}}, main, cpu?, platform?, syntax?, format?, defines?, listing?}`, or multipart/form-data with file parts and text fields. The response holds `ok`, the outputs (base64), diagnostics (file, line, column and rendered text), symbols, prints, the listing and VICE labels. |
| POST | `/disassemble` | `{cpu, org, base64, a16?, i16?}` |
| POST | `/convert` | `{source, name?, from?, to}` |
| GET | `/cpus`, `/platforms`, `/formats`, `/health` | |

Requests only see the files they upload, through an in-memory file
system; the server never touches its own disk. Bodies are limited to
32 MB and each assembly to 30 s.

```sh
curl -s localhost:8080/assemble -H 'content-type: application/json' \
  -d '{"files":{"main.asm":"BasicUpstart2(start)\nstart: inc $d020\n jmp start\n"},"main":"main.asm"}'
```

## Converting between dialects

`rat convert` parses a file and writes the same program in another
dialect:
* Comments and blank lines are kept.
* ACME zones, `.local`/`@cheap` labels and anonymous labels are mapped onto
  Kick's `{}` scopes and `!` multi-labels, and back.
* Operators are re-spelled for the target, for example ACME `=`, `XOR`
  and `^` become `==`, `^` and `pow()`.
* Data byte order follows the CPU.

Constructs the target cannot express are reported as warnings and left
as `TODO(rat convert)` comments. For example, KickAssembler lists,
structs and functions cannot be converted to ACME. The test suite
converts every corpus file to every dialect and checks that the result
assembles to identical bytes.

## Known gaps

* KickAssembler: `.plugin` (Java), `.modify`/`.filemodify` and segment
  modifiers, `createFile`, and label arrays of loops (`loop[i].label`)
  are not supported. The `dtv` CPU is missing because mimic does not
  emulate it.
* `!cpu 65ce02` / `4502` (ACME) are not available, for the same reason.
  `65c02` has no Rockwell/WDC bit instructions (`RMB`, `SMB`, `BBR`,
  `BBS`, `STP`, `WAI`), as mimic's 65C02 (the NCR/GTE part of the
  Apple IIe).
* Macintosh output is raw code (`bin`, `hex`, `srec`); there is no
  resource-fork or MFS disk writer.
* Converting KickAssembler script code (lists, structs, functions,
  pseudocommands, segments) to ACME leaves `TODO(rat convert)` comments.

## How it works

```
crates/
  rat-core      spans, diagnostics, tokens, configurable lexer, statement IR
  rat-cpu       CPU backends (encoder + disassembler per CPU)
  rat-syntax    ACME and Kick/rat parsers → shared IR
  rat-platform  platform registry, Commodore preludes, SID/picture loading
  rat-output    prg, xex, nes, sfc, a26, p, amsdos, hunk, crt, d64, tap, sna, dsk, hex, srec, symbol files
  rat-asm       the multi-pass engine, script values, converter
  rat-server    REST API
  rat-cli       the `rat` binary
tests/          round trips against mimic, corpus with golden outputs
```

**Passes.** The engine runs the IR once per pass. A forward reference
uses the value from the previous pass. Operand sizes, such as zero page
versus absolute or `.s` versus `.w` branches, can only grow, so the passes
converge. Assembly stops when no symbol changes. If values never settle,
the undefined symbols are reported.

**Amiga relocation.** Hunk executables are relocated by assembling again
with one hunk moved: longwords that move with it become RELOC32 entries.

## Testing

```
cargo test --workspace --release
```

* **Round trips against mimic.** `tests/tests/roundtrip.rs` takes every
  6502, 65C02, 65816 (all M/X widths), SPC700, Z80 (all prefix pages) and 6809
  opcode (all postbytes), plus every 68000 opcode word and a third of the
  68020 space. Each one is disassembled with mimic's disassembler,
  assembled with rat, disassembled again and compared. The comparison is
  on text, so it accepts equivalent encodings.
* **Corpus.** `tests/tests/corpus.rs` assembles C64, ACME, Kick script,
  CPC, Vectrex, NES, SNES (with SPC700), Atari 8-bit, 2600 and Amiga
  sources against golden outputs, and checks conversion to every dialect.
* **REST.** `rat-server` has tests for JSON and multipart assembly,
  disassembly, conversion and the OpenAPI document.
