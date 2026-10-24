#define DEBUG
.const BORDER = $d020
.var music = LoadSid("tune.sid")
.segmentdef Code [start=$0810]
.segmentdef Data [start=$2000]
.segment Default
BasicUpstart2(main)
.segment Code
main:   sei
        lda #<irq
        sta $0314
        :SetBorder(5)
        mov #3 : BORDER
        jsr music.init
#if DEBUG
        inc BORDER
#endif
        jmp *
irq:    jsr music.play
        jmp $ea31
.macro SetBorder(color) {
        lda #color
        sta BORDER
}
.pseudocommand mov src : dst {
        lda src
        sta dst
}
.function double(x) { .return x * 2 }
.struct Point {x, y}
.segment Data
.var p = Point(3, 4)
        .byte p.x, p.y, double(21)
.var l = List().add(1, 2, 3)
.eval l.add(4)
        .byte l.size(), l.get(3)
.var h = Hashtable().put("a", 10)
        .byte h.get("a")
sine:   .fill 8, round(127 + 127*sin(toRadians(i*360/8)))
        .lohifill 4, i*$100 + 3
.for (var i=0; i<3; i++) { .byte i }
.namespace ns {
inner:  .byte $42
}
        .word ns.inner
.enum { RED, GREEN=5, BLUE }
        .byte RED, GREEN, BLUE
*= music.location "Music"
        .fill music.size, music.getData(i)
.assert "double", double(2), 4
.print "size=" + music.size + " name=" + music.name
.disk [filename="test.d64", name="RAW DISK", id="21"] {
    [name="MAIN", type="prg", segments="Default,Code,Data"],
}
