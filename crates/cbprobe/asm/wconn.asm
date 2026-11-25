; =============================================================================
; WCONN.COM - Gate 4b.5/4b.6: WPA2-PSK Wi-Fi connect + INT 60h packet driver
; =============================================================================
; Parses SSID and optional passphrase from the DOS command line, allocates a
; 16 KiB DMA block (1024 paragraphs) and packet-driver stub block (sized from
; FERRIC.JLM AX=0x0003, copied via AX=0x0005 with [es:0x000C] patched), and
; calls FERRIC.JLM V86 API AX=0x000C (gate1_wifi_connect).
;
; On success (AX=0): saves old INT 60h via AH=35h/AX=0x0007, points INT 60h
; to stub_seg:0000 via AX=2560h, prints connection status, and exits TSR
; (INT 21h AX=3100h) keeping both allocated DOS blocks resident.
; On failure (AX!=0): frees both DOS blocks (AH=49h), prints failure status,
; and exits via INT 21h AH=4Ch with errorlevel = status.
;
; Strictly 386 baseline, read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

WIFI_DMA_PARAS equ 1024         ; 16 KiB

start:
        push cs
        pop ds
        push cs
        pop es

        ; --- Parse command line from PSP 0x80 (len) / 0x81 (tail) ---
        movzx cx, byte [0x80]
        mov si, 0x81
        call skip_ws
        test cx, cx
        jz .usage
        mov al, [si]
        cmp al, 0x0D
        je .usage
        cmp al, 0x00
        je .usage

        ; Arg 1: SSID (1..=32 bytes) -> req_buf + 0x10
        mov di, req_buf + 0x10
        mov dx, 32
        call parse_token
        jc .usage
        test ax, ax
        jz .usage
        mov [req_buf + 0x00], al

        ; Optional Arg 2: passphrase (8..=63 bytes; omit for open network)
        call skip_ws
        test cx, cx
        jz .args_ok
        mov al, [si]
        cmp al, 0x0D
        je .args_ok
        cmp al, 0x00
        je .args_ok

        mov di, req_buf + 0x30
        mov dx, 63
        call parse_token
        jc .bad_pass
        cmp ax, 8
        jb .bad_pass
        mov [req_buf + 0x01], al

.args_ok:
        ; Shrink our .COM block so DOS AH=48h can allocate DMA + stub blocks.
        push cs
        pop es
        mov bx, (wconn_end - $$ + 0x100 + 15) / 16
        mov ah, 0x4A
        int 0x21

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
        ; Query packet-stub size in paragraphs via AX=0x0003 -> CX
        mov ax, 0x0003
        call far [jlm_off]
        mov [stub_paras], cx
        test cx, cx
        jnz .alloc_dma

        mov si, msg_no_mem
        call print_str
        mov ax, 0x4C02
        int 0x21

.alloc_dma:
        ; Allocate 16 KiB DMA block (1024 paragraphs)
        mov bx, WIFI_DMA_PARAS
        mov ah, 0x48
        int 0x21
        jnc .dma_ok

        mov si, msg_no_mem
        call print_str
        mov ax, 0x4C02
        int 0x21

.dma_ok:
        mov [dma_seg], ax

        ; Allocate packet-stub block ([stub_paras] paragraphs)
        mov bx, [stub_paras]
        mov ah, 0x48
        int 0x21
        jnc .stub_ok

        mov byte [req_buf + 0x02], 2
        jmp .free_fail

.stub_ok:
        mov [stub_seg], ax

        ; Copy packet stub into stub_seg:0000 via AX=0x0005 and patch self-seg
        mov ax, [stub_seg]
        mov es, ax
        xor di, di
        mov ax, 0x0005
        call far [jlm_off]
        mov ax, [stub_seg]
        mov es, ax
        mov [es:0x000C], ax

        ; Call AX=0x000C (gate1_wifi_connect):
        ;   BX = dma_seg, CX = stub_seg, ES:DI = req_buf (160 bytes)
        push cs
        pop es
        mov di, req_buf
        mov bx, [dma_seg]
        mov cx, [stub_seg]
        mov ax, 0x000C
        call far [jlm_off]

        ; The connect call has consumed the request; the passphrase
        ; must not outlive it in memory (see scrub_pass).
        call scrub_pass

        test ax, ax
        jz .connected

        ; Ensure status byte is non-zero if AX != 0
        cmp byte [req_buf + 0x02], 0
        jne .free_fail
        mov [req_buf + 0x02], al
        jmp .free_fail

.connected:
        ; Save incumbent INT 60h vector and record it in FERRIC.JLM (AX=0x0007)
        mov ax, 0x3560
        int 0x21
        mov [old_off], bx
        mov ax, es
        mov [old_seg], ax
        mov bx, [old_off]
        mov cx, [old_seg]
        mov ax, 0x0007
        call far [jlm_off]

        push cs
        pop es

        ; Point INT 60h to stub_seg:0000 via AX=2560h
        push ds
        mov ax, [stub_seg]
        mov ds, ax
        xor dx, dx
        mov ax, 0x2560
        int 0x21
        pop ds

        ; Line 1: WCONN: status=0 ch=C bssid=XX:XX:XX:XX:XX:XX mac=XX-XX-XX-XX-XX-XX
        mov si, msg_status0
        call print_str
        movzx ax, byte [req_buf + 0x03]
        call print_dec16

        mov si, msg_bssid
        call print_str
        mov si, req_buf + 0x04
        mov dl, ':'
        call print_mac

        mov si, msg_mac
        call print_str
        mov si, req_buf + 0x0A
        mov dl, '-'
        call print_mac
        call print_crlf

        ; Line 2: WCONN: CONNECTED (INT 60h -> Wi-Fi)
        mov si, msg_conn
        call print_str

        ; Exit TSR (INT 21h AX=3100h), keeping DMA + stub blocks resident
        mov dx, wconn_end
        add dx, 15
        shr dx, 4
        mov ax, 0x3100
        int 0x21

.free_fail:
        mov ax, [stub_seg]
        test ax, ax
        jz .free_dma
        mov es, ax
        mov ah, 0x49
        int 0x21
.free_dma:
        mov ax, [dma_seg]
        test ax, ax
        jz .print_fail
        mov es, ax
        mov ah, 0x49
        int 0x21
.print_fail:
        push cs
        pop es
        mov si, msg_status
        call print_str
        movzx ax, byte [req_buf + 0x02]
        call print_dec16
        mov si, msg_failed
        call print_str
        mov al, [req_buf + 0x02]
        cmp al, 1
        je .fail_r1
        cmp al, 2
        je .fail_r2
        cmp al, 3
        je .fail_r3
        cmp al, 4
        je .fail_r4
        cmp al, 5
        je .fail_r5
        cmp al, 6
        je .fail_r6
        jmp .fail_eol
.fail_r1:
        mov si, msg_r1
        call print_str
        jmp .fail_eol
.fail_r2:
        mov si, msg_r2
        call print_str
        jmp .fail_eol
.fail_r3:
        mov si, msg_r3
        call print_str
        jmp .fail_eol
.fail_r4:
        mov si, msg_r4
        call print_str
        jmp .fail_eol
.fail_r5:
        mov si, msg_r5
        call print_str
        jmp .fail_eol
.fail_r6:
        mov si, msg_r6
        call print_str
.fail_eol:
        call print_crlf
        mov al, [req_buf + 0x02]
        mov ah, 0x4C
        int 0x21

.usage:
        call scrub_pass
        mov si, msg_usage
        call print_str
        mov ax, 0x4C01
        int 0x21

.bad_pass:
        call scrub_pass
        mov si, msg_badpass
        call print_str
        mov ax, 0x4C01
        int 0x21

; Zero the passphrase everywhere it was copied: the request buffer
; field it was parsed into and the PSP command tail it was parsed
; from. On success this program stays resident as a TSR, so an
; unscrubbed buffer sits in DOS memory for the life of the machine,
; readable by anything that walks the memory chain; the tail is
; also what shells and debuggers echo back. The SSID is not secret
; and is left alone.
scrub_pass:
        push ax
        push cx
        push di
        push es
        push cs
        pop es
        xor ax, ax
        mov di, req_buf + 0x30
        mov cx, 64
        rep stosb
        ; Both regions live in this program's own segment (the PSP
        ; of a .COM is at CS:0); ES addresses them so the routine
        ; does not depend on DS surviving the JLM call.
        movzx cx, byte [es:0x80]
        mov di, 0x81
        rep stosb
        pop es
        pop di
        pop cx
        pop ax
        ret

; --- Command-line parser helpers ---
skip_ws:
        test cx, cx
        jz .done
        mov al, [si]
        cmp al, ' '
        je .next
        cmp al, 0x09
        jne .done
.next:
        inc si
        dec cx
        jmp skip_ws
.done:
        ret

; Inputs:  SI=src, CX=remaining, DI=dst, DX=max_len
; Outputs: AX=length copied, CF=0 ok / CF=1 overflow; SI and CX updated
parse_token:
        push bx
        xor bx, bx
        test cx, cx
        jz .ok
        mov al, [si]
        cmp al, 0x0D
        je .ok
        cmp al, 0x00
        je .ok
        cmp al, '"'
        je .quoted

.unquoted_loop:
        test cx, cx
        jz .ok
        mov al, [si]
        cmp al, 0x0D
        je .ok
        cmp al, 0x00
        je .ok
        cmp al, ' '
        je .ok
        cmp al, 0x09
        je .ok
        cmp bx, dx
        jae .overflow
        mov [di], al
        inc di
        inc bx
        inc si
        dec cx
        jmp .unquoted_loop

.quoted:
        inc si
        dec cx
.quoted_loop:
        test cx, cx
        jz .ok
        mov al, [si]
        cmp al, 0x0D
        je .ok
        cmp al, 0x00
        je .ok
        cmp al, '"'
        je .quoted_end
        cmp bx, dx
        jae .overflow
        mov [di], al
        inc di
        inc bx
        inc si
        dec cx
        jmp .quoted_loop
.quoted_end:
        inc si
        dec cx
        jmp .ok

.overflow:
        mov ax, bx
        pop bx
        stc
        ret

.ok:
        mov ax, bx
        pop bx
        clc
        ret

; --- Output helpers ---
; Print 6-byte MAC/BSSID at DS:SI separated by DL
print_mac:
        push ax
        push cx
        push dx
        push si
        mov cx, 6
.loop:
        mov al, [si]
        call print_hex8
        inc si
        dec cx
        jz .done
        mov al, dl
        call print_char
        jmp .loop
.done:
        pop si
        pop dx
        pop cx
        pop ax
        ret

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
        mov si, msg_crlf
        call print_str
        ret

print_dec16:
        push ax
        push bx
        push cx
        push dx
        mov bx, 10
        xor cx, cx
.div:
        xor dx, dx
        div bx
        push dx
        inc cx
        test ax, ax
        jnz .div
.out:
        pop dx
        add dl, '0'
        mov al, dl
        call print_char
        loop .out
        pop dx
        pop cx
        pop bx
        pop ax
        ret

print_hex8:
        push ax
        push cx
        mov cl, al
        shr al, 4
        call print_nibble
        mov al, cl
        and al, 0x0F
        call print_nibble
        pop cx
        pop ax
        ret

print_nibble:
        and al, 0x0F
        add al, '0'
        cmp al, '9'
        jbe .ok
        add al, 7
.ok:
        call print_char
        ret

; --- Data ---
jlm_off:    dw 0
jlm_seg:    dw 0
stub_paras: dw 0
dma_seg:    dw 0
stub_seg:   dw 0
old_off:    dw 0
old_seg:    dw 0

msg_usage:   db 'Usage: WCONN "SSID" [passphrase]', 13, 10, '$'
msg_badpass: db 'WCONN: passphrase must be 8..63 chars (omit for open)', 13, 10, '$'
msg_no_jlm:  db 'WCONN: ERROR FERRIC.JLM not loaded (boot Profile 1 or 2)', 13, 10, '$'
msg_no_mem:  db 'WCONN: ERROR DOS memory alloc failed', 13, 10, '$'
msg_status0: db 'WCONN: status=0 ch=$'
msg_bssid:   db ' bssid=$'
msg_mac:     db ' mac=$'
msg_conn:    db 'WCONN: CONNECTED (INT 60h -> Wi-Fi)', 13, 10, '$'
msg_status:  db 'WCONN: status=$'
msg_failed:  db ' (FAILED)$'
msg_r1:      db ' - no AR2413 card/EEPROM$'
msg_r2:      db ' - DMA lock or radio init failed$'
msg_r3:      db ' - SSID not found$'
msg_r4:      db ' - 802.11 auth timeout$'
msg_r5:      db ' - 802.11 assoc timeout$'
msg_r6:      db ' - WPA2 handshake failed$'
msg_crlf:    db 13, 10, '$'

; 160-byte WifiConnectReq / WifiConnectResp struct:
;   +0x00: ssid_len (u8)
;   +0x01: pass_len (u8)
;   +0x02: status (u8)
;   +0x03: channel (u8)
;   +0x04..+0x09: bssid ([u8; 6])
;   +0x0A..+0x0F: sta_mac ([u8; 6])
;   +0x10..+0x2F: ssid ([u8; 32])
;   +0x30..+0x6F: passphrase ([u8; 64])
;   +0x70..+0x9F: reserved (48 bytes)
req_buf:    times 160 db 0

wconn_end:
