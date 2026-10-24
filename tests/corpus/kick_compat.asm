// KickAssembler compatibility (manual examples)
*=$1000
start:  lda #0; sta $d020; sta $d021
        lda #BLACK
        ldx #LIGHT_GREY
        .byte LDA_IMM, JMP_IND, RTS, asmCommandSize(LDA_ABS)
        lax #$12
        .fill 4, [$10, $20]
        .var r = random()
        .errorif r == random(), "random() repeats"
.for (var i=0; i<2; i++)
        .byte i
.var x = 3
.if (x > 2) lda #1 else lda #2
.label @rootLabel = $c000
        .word @rootLabel
        .text "ab" + 'c'
.var h = Hashtable().put("a", 1, "b", 2)
        .byte h.get("b"), h.keys().size()
.var m = Matrix()
.eval m.set(0, 0, 7)
        .byte m.get(0, 0)
.assert "same code", { lda #1 }, { .byte $a9, 1 }
.asserterror "fails", 20/false
tmpX:   ldx tmpX2:#$00
        stx tmpX2
.function next(arg) {
    .if (arg.getType() == AT_IMMEDIATE)
        .return CmdArgument(arg.getType(), >arg.getValue())
    .return CmdArgument(arg.getType(), arg.getValue() + 1)
}
.pseudocommand mov16 src : tar {
        lda src
        sta tar
        lda next(src)
        sta next(tar)
}
        mov16 #$1234 : $0314
c2:     setColor()
        inc c2.color
.macro setColor() {
        .label color = *+1
        lda #0
}
myIf: .if (true) {
label:  nop
}
        jmp myIf.label
*=$2000 "Virtual" virtual
virt:   .byte 1, 2, 3
