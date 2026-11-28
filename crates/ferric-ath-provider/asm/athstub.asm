; =============================================================================
; PKTSTUB.BIN - real-mode half of the Ferric DOS packet driver (Gate 2, 2.3)
; =============================================================================
; The packet-driver contract is a real-mode contract: clients find the
; driver by the "PKT DRVR" signature at offset 3 of the INT 60h handler
; and call it with a far call through the vector. The driver proper
; lives in the protected-mode JLM, which cannot own a real-mode vector
; (a Jemm chain hook's code is not what the vector points at, so the
; signature check would fail). This stub is the bridge: the JLM copies
; it into a small self-owned DOS block at attach, points INT 60h at it,
; and the stub forwards every call to the JLM through INT 0F1h, a
; vector the JLM does hook.
;
; The stub is position-independent within its segment and touches no
; API register: the bridge interrupt returns the API results in the
; registers themselves, and the result carry is copied into the
; caller's stacked FLAGS image (the word an IRET will load), which is
; the only honest way an interrupt handler returns a flag.
;
; Save discipline: the packet-driver contract lets a client send from
; inside its receive upcall, so a dispatch can nest inside another
; dispatch's bridge call. The caller's SS:SP and the result carry
; therefore live on the stub's own stack across the bridge call, and
; the stub stack pointer persists between invocations (stub_sp), so
; a nested dispatch stacks its frame below the outer one instead of
; restarting at stack_top and overwriting it. The saved_ss/saved_sp/
; result_cf cells are transit only: written and consumed inside the
; entry and exit sequences, never held across the bridge call.
; =============================================================================

bits 16
org 0

BRIDGE_INT  equ 0xF1
STUB_SEG_OFF equ 0x0C           ; word patched by the installer

start:
    jmp short dispatch          ; +0x00
    nop                         ; +0x02
    db 'PKT DRVR', 0            ; +0x03 signature, read by clients at seg:3

stub_seg:   dw 0                ; +0x0C our own segment (patched at install)
saved_ss:   dw 0                ; +0x0E transit cell, entry/exit only
saved_sp:   dw 0                ; +0x10 transit cell, entry/exit only
result_cf:  dw 0                ; +0x12 transit cell, exit only (FLAGS image)
stub_sp:    dw stack_top        ; +0x14 where the stub stack rests between
                                ;       invocations; frames stack downward
            dw 0, 0, 0, 0, 0    ; +0x16 reserved

drv_name:                       ; +0x20 driver_info returns DS:SI here
    db 'Ferric DOS AR2413 packet driver', 0

    times (0x50 - ($ - $$)) db 0

dispatch:
    mov [cs:saved_ss], ss
    mov [cs:saved_sp], sp
    mov ss, [cs:stub_seg]       ; MOV SS inhibits interrupts one insn,
    mov sp, [cs:stub_sp]        ; so the pair switches stacks atomically.
                                ; SP resumes where the last invocation
                                ; left it, not at stack_top: a nested
                                ; dispatch must stack below the frame
                                ; of the dispatch it nests inside.
    push word [cs:saved_ss]     ; caller's stack address now lives on
    push word [cs:saved_sp]     ; our stack across the bridge call
    int BRIDGE_INT              ; JLM implements the API; results in regs + CF
    pushf                       ; park the result FLAGS on our stack too
    pop word [cs:result_cf]
    pop word [cs:saved_sp]
    pop word [cs:saved_ss]
    mov [cs:stub_sp], sp        ; hand the resting stack pointer back
    mov ss, [cs:saved_ss]
    mov sp, [cs:saved_sp]
    ; Back on the caller's stack: [sp]=IP [sp+2]=CS [sp+4]=FLAGS.
    ; Copy the result carry into the stacked FLAGS the IRET will load.
    push bp
    mov bp, sp                  ; [bp+2]=IP [bp+4]=CS [bp+6]=FLAGS
    test byte [cs:result_cf], 0x01
    jz .clr
    or word [bp+6], 0x0001
    jmp .done
.clr:
    and word [bp+6], 0xFFFE
.done:
    pop bp
    iret

    times 192 db 0
stack_top:
