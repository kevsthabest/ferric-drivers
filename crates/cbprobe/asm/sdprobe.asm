; =============================================================================
; SDPROBE.COM - Toshiba TC6371AF / Type-A Built-in SD Slot Probe (Read-Only)
; (JLM verbs 0x003C init / 0x003D read_sec / 0x003E shutdown in FERRIC.JLM)
; =============================================================================
; Usage:
;   SDPROBE          Probe 1179:0805 SD slot, identify card, read sector 0
;   SDPROBE /R <lba> Read and hex-dump 512-byte sector <lba> (decimal)
;   SDPROBE /U       Power down SD slot cleanly
;
; Strictly 386 baseline, strictly read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

start:
        push cs
        pop ds
        push cs
        pop es

        ; Prefer standalone TOSHSD.JLM (0xFD06); fall back to FERRIC.JLM (0xFD01).
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD06
        int 0x2F
        mov [jlm_off], di
        mov ax, es
        mov [jlm_seg], ax
        or ax, di
        jnz .have_jlm

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

        ; Parse PSP command tail for /U or /R <lba>
        movzx cx, byte [0x0080]
        mov si, 0x0081
        xor bl, bl                      ; 0 = init, 1 = /U, 2 = /R
.scan:
        test cx, cx
        jz .scan_done
        lodsb
        dec cx
        cmp al, ' '
        je .scan
        cmp al, 9
        je .scan
        cmp al, 13
        je .scan_done
        cmp al, '/'
        je .opt
        cmp al, '-'
        je .opt
        jmp .scan
.opt:
        test cx, cx
        jz .scan_done
        lodsb
        dec cx
        or al, 0x20
        cmp al, 'u'
        je .set_u
        cmp al, 'r'
        je .set_r
        jmp .scan
.set_u:
        mov bl, 1
        jmp .scan
.set_r:
        mov bl, 2
        ; Parse decimal LBA following /R
        xor edx, edx
.skip_sp:
        test cx, cx
        jz .store_lba
        mov al, [si]
        cmp al, ' '
        je .adv_sp
        cmp al, 9
        je .adv_sp
        cmp al, ':'
        je .adv_sp
        jmp .parse_dec
.adv_sp:
        inc si
        dec cx
        jmp .skip_sp
.parse_dec:
        test cx, cx
        jz .store_lba
        mov al, [si]
        cmp al, '0'
        jb .store_lba
        cmp al, '9'
        ja .store_lba
        inc si
        dec cx
        sub al, '0'
        movzx eax, al
        imul edx, edx, 10
        add edx, eax
        jmp .parse_dec
.store_lba:
        mov [req_lba], edx
        jmp .scan_done

.scan_done:
        cmp bl, 1
        je .do_shutdown
        cmp bl, 2
        je .do_read

        ; Default: AX=0x003C (gate1_sd_init)
        mov di, io_buf
        mov cx, 2048
        mov ax, 0x003C
        call far [jlm_off]
        push cs
        pop es
        call print_report
        mov ax, 0x4C00
        int 0x21

.do_shutdown:
        mov ax, 0x003E
        call far [jlm_off]
        push cs
        pop es
        mov si, msg_shut
        call print_str
        mov ax, 0x4C00
        int 0x21

.do_read:
        ; First ensure card is initialized (silent if already ready, or prints init)
        mov di, io_buf
        mov cx, 2048
        mov ax, 0x003C
        call far [jlm_off]
        push cs
        pop es
        call print_report

        ; Read requested LBA: AX=0x003D, EBX=lba, ES:DI=sec_buf
        mov di, sec_buf
        mov ebx, [req_lba]
        mov ax, 0x003D
        call far [jlm_off]
        push cs
        pop es
        test ax, ax
        jz .read_ok
        mov si, msg_read_err
        call print_str
        call print_hex16
        call print_crlf
        mov ax, 0x4C02
        int 0x21

.read_ok:
        ; Hex + ASCII dump first 256 bytes (16 rows x 16 bytes)
        mov si, sec_buf
        xor bp, bp                      ; offset 0..240
.dump_row:
        mov ax, bp
        call print_hex16
        mov al, ':'
        call print_char
        mov al, ' '
        call print_char
        push si
        mov cx, 16
.dump_hex:
        lodsb
        call print_hex8
        mov al, ' '
        call print_char
        loop .dump_hex
        pop si
        mov al, ' '
        call print_char
        mov cx, 16
.dump_asc:
        lodsb
        cmp al, 0x20
        jb .dot
        cmp al, 0x7E
        ja .dot
        jmp .emit_asc
.dot:
        mov al, '.'
.emit_asc:
        call print_char
        loop .dump_asc
        call print_crlf
        add bp, 16
        cmp bp, 256
        jb .dump_row

        mov ax, 0x4C00
        int 0x21

; Print AX bytes from io_buf
print_report:
        mov cx, ax
        test cx, cx
        jz .done
        mov si, io_buf
.ch:
        lodsb
        call print_char
        loop .ch
.done:
        ret

print_hex16:
        push ax
        mov al, ah
        call print_hex8
        pop ax
print_hex8:
        push ax
        shr al, 4
        call print_nib
        pop ax
        and al, 0x0F
print_nib:
        cmp al, 9
        jbe .d
        add al, 'A' - 10
        jmp print_char
.d:
        add al, '0'
        jmp print_char

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

print_crlf:
        mov al, 13
        call print_char
        mov al, 10
        jmp print_char

jlm_off:      dw 0
jlm_seg:      dw 0
req_lba:      dd 0

msg_no_jlm:   db 'SDPROBE: FERRIC.JLM (0xFD01) not loaded', 13, 10, '$'
msg_shut:     db 'SDPROBE: Toshiba SD slot powered down.', 13, 10, '$'
msg_read_err: db 'SDPROBE: sector read failed, err=0x$'

sec_buf:      times 512 db 0
io_buf:       times 2048 db 0
