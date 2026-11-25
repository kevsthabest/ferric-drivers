; =============================================================================
; EHCPROBE.COM - Stage H: EHCI (USB 2.0) recon and bring-up client
; (JLM verbs 0x0036 recon / 0x0037 bring-up in FERRIC.JLM)
; =============================================================================
; Usage:
;   EHCPROBE        Print the EHCI capability recon report only.
;   EHCPROBE /B     Also bring the controller up (BIOS handoff, reset,
;                   async schedule, port routing) and print the status
;                   report. The async schedule lives in a 16 KiB DMA
;                   block this program allocates, so /B stays resident
;                   (TSR) to keep that block owned; a plain recon run
;                   exits normally.
;
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
        push cs
        pop es

        ; Scan the PSP command tail for /B (bring-up) or /U (shutdown).
        movzx cx, byte [0x0080]
        mov si, 0x0081
        xor bx, bx                      ; BL = mode: 0=recon, 1=/B, 2=/U
.tail_scan:
        test cx, cx
        jz .tail_done
        lodsb
        dec cx
        cmp al, ' '
        je .tail_scan
        cmp al, 9
        je .tail_scan
        cmp al, 13
        je .tail_done
        cmp al, '/'
        je .maybe_opt
        cmp al, '-'
        jne .skip_token
.maybe_opt:
        test cx, cx
        jz .tail_done
        lodsb
        dec cx
        or al, 0x20
        cmp al, 'b'
        je .set_b
        cmp al, 'u'
        je .set_u
        jmp .skip_token
.set_b:
        mov bl, 1
        jmp .skip_token
.set_u:
        mov bl, 2
        jmp .skip_token
.skip_token:
        test cx, cx
        jz .tail_done
        lodsb
        dec cx
        cmp al, ' '
        je .tail_scan
        cmp al, 9
        je .tail_scan
        cmp al, 13
        je .tail_done
        jmp .skip_token
.tail_done:
        mov [bringup_flag], bl

        cmp byte [bringup_flag], 2
        je .do_shutdown

        ; Recon report: AX=0x0036, ES:DI = io_buf, CX = capacity.
        mov di, io_buf
        mov cx, 2048
        mov ax, 0x0036
        call far [jlm_off]
        push cs
        pop es
        call print_report

        cmp byte [bringup_flag], 1
        je .do_bringup
        mov ax, 0x4C00
        int 0x21

.do_shutdown:
        mov ax, 0x0038
        call far [jlm_off]
        push cs
        pop es
        or ax, ax
        jz .shut_inactive
        mov si, msg_shut_ok
        call print_str
        mov ax, 0x4C00
        int 0x21
.shut_inactive:
        mov si, msg_shut_none
        call print_str
        mov ax, 0x4C00
        int 0x21

.do_bringup:
        ; Shrink our own allocation so AH=48h can hand out the 16 KiB
        ; DMA block (same pattern as USBPROBE). The whole image,
        ; io_buf included, fits in the shrunk 4 KiB.
        mov sp, 0x0FFE
        mov bx, 256
        mov ah, 0x4A
        int 0x21

        mov bx, 1024                    ; 16 KiB in paragraphs
        mov ah, 0x48
        int 0x21
        jnc .dma_ok
        mov si, msg_no_mem
        call print_str
        mov ax, 0x4C02
        int 0x21
.dma_ok:
        mov [dma_seg], ax

        ; Bring-up + status: AX=0x0037, BX = DMA segment,
        ; ES:DI = io_buf, CX = capacity. Returns AX = report length,
        ; BX = 1 iff freshly brought up (must stay resident).
        push cs
        pop es
        mov di, io_buf
        mov cx, 2048
        mov bx, [dma_seg]
        mov ax, 0x0037
        call far [jlm_off]
        push cs
        pop es
        push bx                         ; save fresh_bringup flag
        call print_report
        pop bx

        or bx, bx
        jnz .stay_resident
        ; Not freshly brought up (absent, failed, or already up):
        ; release the unused 16 KiB DMA block and exit normally.
        mov es, [dma_seg]
        mov ah, 0x49
        int 0x21
        mov ax, 0x4C00
        int 0x21

.stay_resident:
        ; Stay resident: the EHCI async schedule lives in the DMA
        ; block, so the block (and this program, its owner) must not
        ; be freed. DX = resident size in paragraphs.
        mov dx, ehcprobe_end
        add dx, 15
        shr dx, 4
        mov ax, 0x3100
        int 0x21

; Print AX bytes from io_buf (the verb reports are CRLF text).
print_report:
        mov cx, ax
        test cx, cx
        jz .pr_done
        mov si, io_buf
.pr_ch:
        lodsb
        call print_char
        loop .pr_ch
.pr_done:
        ret

; --- Output helpers ---
print_str:
        push ax
        push dx
        mov dx, si
        mov ah, 0x09
        int 0x21
        pop dx
        pop ax
        ret

print_char:
        push ax
        push dx
        mov dl, al
        mov ah, 0x02
        int 0x21
        pop dx
        pop ax
        ret

jlm_off:      dw 0
jlm_seg:      dw 0
dma_seg:      dw 0
bringup_flag: db 0

msg_no_jlm:   db 'EHCPROBE: FERRIC.JLM (0xFD01) not loaded', 13, 10, '$'
msg_no_mem:   db 'EHCPROBE: ERROR 16 KiB DMA allocation failed', 13, 10, '$'
msg_shut_ok:  db 'EHCPROBE: EHCI shutdown (CONFIGFLAG=0)', 13, 10, '$'
msg_shut_none: db 'EHCPROBE: EHCI was not active', 13, 10, '$'

io_buf:       times 2048 db 0
ehcprobe_end:
