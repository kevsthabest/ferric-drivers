; =============================================================================
; USBEJECT.COM - Gate 3: Safe Eject Utility for USB Mass Storage
; =============================================================================
; Usage:
;   USBEJECT
;
; Flushes all FreeDOS disk buffers (INT 21h AH=0Dh), then calls FERRIC.JLM
; V86 API AX=0x0019 (SCSI SYNCHRONIZE CACHE(10) + START STOP UNIT LoEj=1,
; Start=0 + volume invalidation) so the USB flash drive can be unplugged safely.
; Strictly 386 baseline.
; =============================================================================

cpu 386
bits 16
org 0x0100

start:
        push cs
        pop ds
        push cs
        pop es

        ; Discover FERRIC.JLM via INT 2Fh AX=1684h BX=0xFD01
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD01
        int 0x2F
        mov [jlm_off], di
        mov ax, es
        mov [jlm_seg], ax
        or ax, di
        jnz .have_jlm

        mov si, msg_no_jlm
        call print_str
        mov ax, 0x4C01
        int 0x21

.have_jlm:
        ; Flush all FreeDOS dirty filesystem buffers first
        mov ah, 0x0D
        int 0x21

        ; Call FERRIC.JLM AX=0x0019 (gate1_usb_msc_eject)
        mov ax, 0x0019
        call far [jlm_off]
        test ax, ax
        jnz .ok

        mov si, msg_no_msc
        call print_str
        mov ax, 0x4C02
        int 0x21

.ok:
        mov si, msg_ok
        call print_str
        mov ax, 0x4C00
        int 0x21

print_str:
        push ax
        push dx
        mov dx, si
        mov ah, 0x09
        int 0x21
        pop dx
        pop ax
        ret

jlm_off:    dw 0
jlm_seg:    dw 0

msg_no_jlm: db 'USBEJECT: ERROR FERRIC.JLM not loaded', 13, 10, '$'
msg_no_msc: db 'USBEJECT: No USB storage drive mounted (or already ejected)', 13, 10, '$'
msg_ok:     db 'USBEJECT: USB drive flushed and safely ejected. Safe to unplug.', 13, 10, '$'
