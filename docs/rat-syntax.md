# The rat dialect

rat's own dialect is meant for every CPU, not just the 6502. Use it with
the extension `.rat`, or put `.syntax rat` at the top of the file. It reads
like KickAssembler, minus the C64 assumptions, plus a few conveniences
from ca65, vasm and Devpac.

```
; comments: `;` or `//`, block comments /* ... */
.platform nes                 ; CPU, origin, formats, directives
.ines mapper=0, prg=1, chr=1, mirroring=v

.org $c000
reset:  sei
        ldx #$ff
        txs
@wait:  bit $2002             ; @local / .local: local to the last global label
        bpl @wait
-       jmp -                 ; anonymous labels like ACME
.org $fffa
        .word nmi, reset, nmi
```

## Labels and symbols

| Syntax | Meaning |
|---|---|
| `name:` | label at the program counter |
| `@name:` / `.name:` | cheap local label, visible until the next global label |
| `-` `--` `+` `++` | anonymous labels, referenced as `-`, `+`, … (as in ACME) |
| `name = expr` | constant label |
| `.const x = e`, `.var x = e`, `.set x = e`, `.label x = e` | constant, variable (`.eval x = …` changes it), label |
| `name: { … }`, `.scope name { … }` | named scope; inner labels are reached as `name.inner` |
| `.namespace ns { … }` | namespace |

## Numbers, strings, operators

* **Numbers:** `$ff`, `0xff`, `%1010`, `0b1010`, `0o17`, `123`, `1.5`, `'a'`.
* **Strings:** `"..."` with `\n \t \" \\ \0 \$xx` escapes.
* **Operators** (C-like, loosest binding first):
  * `?:`, then `||`, `&&`, `|`, `^`, `&`
  * `== !=`, then `< <= > >=`
  * `<< >> >>>`, then `+ -`, then `* / %`, then `**`
  * unary `- ! ~`
* **`<` and `>` at the start of an expression** take the low and high byte
  of everything after them: `#<table+1` is `lo(table+1)`.
* **`/`** is integer division when both sides are integers.
* **PC:** `*` or `$`.
* **Lists and indexing:** `[1, 2, 3]` builds a list; `list[i]` reads an
  element.
* **Functions:**
  * Math: `lo hi bank`, `abs min max floor ceil round sqrt sin cos tan pow …`
  * Strings: `toIntString toHexString toBinaryString`, `len`, `defined(name)`
  * Constructors: `List() Hashtable() Vector() Matrix()`
* **Methods**, as in KickAssembler: `list.add() .get() .size()`,
  `hash.put() .get() .keys()`, `string.substring() .toUpperCase() …`

## Data

| Directive | |
|---|---|
| `.byte`/`.db`, `.word`/`.dw`, `.long`/`.dword`, `.quad` | in the CPU's byte order (`.endian le\|be` overrides) |
| `.le16 .be16 .le24 .be24 .le32 .be32` | explicit byte order |
| `dc.b dc.w dc.l`, `ds.b ds.w ds.l n[, fill]` | 68k style |
| `.text "…"`, `.asciiz "…"`, `.petscii "…"`, `.screen "…"` | text in the current encoding, NUL-terminated, PETSCII, C64 screen codes |
| `.encoding "ascii\|petscii_mixed\|petscii_upper\|screencode_mixed\|screencode_upper\|atari_screen\|zx81\|apple"` | text encoding (the default comes from the platform); `apple` is ASCII with bit 7 set (Apple II `COUT`) |
| `.hex 0123 abcd` | hex bytes |
| `.fill n, expr` | `expr` may use `i`, the index (as in Kick) |
| `.lohifill n, expr` | low and high byte tables, reached as `label.lo` / `label.hi` |
| `.res n` / `.ds n` | n zero bytes |
| `.align n[, fill]`, `.even` | padding |
| `.incbin "file"[, offset[, length]]` | binary include |

## Program counter and memory

| Directive | |
|---|---|
| `.org addr`, `*= addr`, `.pc = addr "block name"` | set the PC (starts a new memory block) |
| `.pseudopc addr { … }` / `.pseudopc addr` … `.realpc` | assemble for another address |
| `.segmentdef`, `.segment`, `.segmentout`, `.file`, `.memblock` | KickAssembler segments |
| `.output "file"[, format]` | main output file and format |

## Control flow and reuse

```
.if x > 3 { … } .elif x { … } .else { … }
.ifdef NAME { … } .else { … }
.for i in 0..8 { .byte i * 2 }          ; exclusive; 0..=8 is inclusive; `step n`
.for v in [1, 5, 9] { .byte v }
.while n < 10 { .eval n = n + 1 }
.repeat 4 { nop }
.macro poke(addr, value = 0) { lda #value
                               sta addr }
poke($d020, 1)        ; or: poke $d020, 1
.fn double(x) = x * 2
.fn clamp(x) {
    .if x > 255 { .return 255 }
    .return x
}
.pseudocommand mov src : dst { lda src
                               sta dst }
mov #1 : $d020
.struct Point {x, y}
.enum {RED, GREEN = 5, BLUE}
#define DEBUG
#if DEBUG
  …
#endif
```

`;` starts a comment in rat, so C-style `for (…; …; …)` loops cannot be
written. Use `.for i in a..b`, or `.while` with `.eval`.

## CPU, platform and target directives

| Directive | |
|---|---|
| `.cpu 6502\|6502ni\|6510\|2a03\|6507\|65c02\|65816\|spc700\|z80\|6809\|68000\|68020` | change the CPU |
| `.platform name [{ … }]` | change the platform; also selects its CPU |
| `.a8 .a16 .i8 .i16` | 65816 register widths |
| `.setdp page` | 6809 direct page; 65816 assumed D register |
| `.endian le\|be` | byte order of data |
| `.include "file"`, `.once`, `.eof` | source files |
| `.print`, `.warn`, `.error`, `.errorif c, msg`, `.assert cond[, msg]`, `.asserterror` | messages and checks |
| `.break`, `.watch` | breakpoints for the VICE label file |
| `.syntax rat` | dialect switch (first line) |

Platform directives are written `.name …` while their platform is active,
or `.<platform>.<name> …` anywhere, for example `.c64.basicupstart start`
or `.xex.run start`. `rat platforms` lists them.

## Operand syntax per CPU

Operands always use the CPU's native syntax.

| CPU | Examples |
|---|---|
| 65xx | `lda #<x`, `lda (zp),y`, `sta $1234,x`. Force a size with `lda.z` / `lda.a` (ACME: `lda+1` / `lda+2`) |
| 65816 | `lda [dp],y`, `lda $12,s`, `lda ($12,s),y`, `jml $123456`, `mvn $7e,$00`, `lda.l`, `pea #x` |
| SPC700 | `mov a,!abs+x`, `mov a,[dp+x]`, `mov a,[dp]+y`, `mov (x)+,a`, `set1 dp.3`, `mov1 c,abs.5`, `bbs dp.2,label`, `mov dst,src` |
| Z80 | `ld a,(ix+5)`, `ld (iy-2),$12`, `res 3,(ix+1),b`, `out (c),0`, `ex af,af'`, `ixh`/`ixl`, `sll` |
| 6809 | `lda <$20` (direct), `lda >$1234` (extended), `ldd ,x++`, `lda [,y]`, `leax 10,x`, `ldy label,pcr`, `pshs a,b,x` |
| 68000/68020 | `move.l #1,d0`, `lea 4(a0,d1.w*2),a1`, `(d16,pc)`, `movem.l d0-d7/a0-a6,-(sp)`, `bra.s`, `$dff180.l`, `([bd,a0],d1.w*4,od)`, `bfextu d0{4:8},d1` |
