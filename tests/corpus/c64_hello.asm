// KickAssembler hello world
BasicUpstart2(start)

        *= $0810 "Main"
start:  ldx #0
!loop:  lda text,x
        beq !done+
        jsr $ffd2
        inx
        bne !loop-
!done:  rts

text:   .encoding "petscii_mixed"
        .text "hello world"
        .byte 13, 0
.print "start=" + toHexString(start)
