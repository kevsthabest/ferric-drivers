; =============================================================================
; WSCAN.COM - Gate 4b step 4b.4b: Atheros AR2413 2.4 GHz live beacon scanner
; =============================================================================
; Shrinks its DOS block, allocates a 16 KiB conventional-memory DMA block
; (1024 paragraphs), calls FERRIC.JLM V86 API AX=0x000B (gate1_wifi_scan) to
; bring up the AR2413 RF2413 radio across 2.4 GHz channels 1..11, capture
; beacons into the VDMAD-locked RX ring, decode them via crates/wifi-core,
; frees the DMA block, and prints all discovered APs with WSCAN: markers.
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

        ; Shrink our .COM block so DOS AH=48h can allocate the 16 KiB DMA block.
        mov bx, (wscan_end - $$ + 0x100 + 15) / 16
        mov ah, 0x4A
        int 0x21

        mov si, msg_title
        call print_str

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
        ; Allocate 16 KiB DMA block
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

        ; Call AX=0x000B: BX=dma_seg, ES:DI=scan_buf -> AX=ap_count
        push cs
        pop es
        mov di, scan_buf
        mov bx, [dma_seg]
        mov ax, 0x000B
        call far [jlm_off]

        ; Free DMA block immediately after scan completes
        mov ax, [dma_seg]
        mov es, ax
        mov ah, 0x49
        int 0x21

        push cs
        pop es

        cmp byte [scan_buf + 0x00], 0
        je .absent

        ; Summary line:
        ; WSCAN: present=1 stage=S rx=R ok=O beacons=B aps=A
        mov si, msg_sum1
        call print_str
        movzx ax, byte [scan_buf + 0x01]
        call print_dec16
        mov si, msg_rx
        call print_str
        mov eax, [scan_buf + 0x04]
        call print_dec32
        mov si, msg_ok
        call print_str
        mov eax, [scan_buf + 0x08]
        call print_dec32
        mov si, msg_bcn
        call print_str
        mov eax, [scan_buf + 0x0C]
        call print_dec32
        mov si, msg_aps
        call print_str
        movzx ax, byte [scan_buf + 0x02]
        call print_dec16
        call print_crlf

        ; Print each AP entry. The count byte comes from the service;
        ; clamp it to the entries scan_buf actually holds (16 entries
        ; of 44 bytes from +0x10) so a bad count can never walk this
        ; loop past the buffer and print memory as fake APs.
        movzx cx, byte [scan_buf + 0x02]
        cmp cx, 16
        jbe .count_ok
        mov cx, 16
.count_ok:
        test cx, cx
        jz .done
        mov word [cur_idx], 0
        mov bx, scan_buf + 0x10
.ap_loop:
        push cx
        push bx

        mov si, msg_ap
        call print_str
        mov ax, [cur_idx]
        call print_dec16
        mov si, msg_ch
        call print_str
        movzx ax, byte [bx + 0x06]
        call print_dec16
        mov si, msg_rssi
        call print_str
        movzx ax, byte [bx + 0x07]
        call print_dec16

        mov si, msg_sec
        call print_str
        cmp byte [bx + 0x08], 0
        jne .sec_wpa2
        cmp byte [bx + 0x09], 0
        jne .sec_wep
        mov si, msg_open
        jmp .sec_print
.sec_wpa2:
        mov si, msg_wpa2
        jmp .sec_print
.sec_wep:
        mov si, msg_wep
.sec_print:
        call print_str

        mov si, msg_bssid
        call print_str
        mov si, bx
        mov cx, 6
.bssid_loop:
        mov al, [si]
        call print_hex8
        inc si
        dec cx
        jz .bssid_done
        mov al, ':'
        call print_char
        jmp .bssid_loop
.bssid_done:

        mov si, msg_ssid
        call print_str
        movzx cx, byte [bx + 0x0A]
        test cx, cx
        jz .ssid_hidden
        cmp cx, 32
        jbe .ssid_len_ok
        mov cx, 32
.ssid_len_ok:
        lea si, [bx + 0x0C]
.ssid_loop:
        mov al, [si]
        cmp al, 0x20
        jb .ssid_dot
        cmp al, 0x7E
        jbe .ssid_emit
.ssid_dot:
        mov al, '.'
.ssid_emit:
        call print_char
        inc si
        loop .ssid_loop
        jmp .ap_eol
.ssid_hidden:
        mov si, msg_hidden
        call print_str
.ap_eol:
        call print_crlf

        pop bx
        pop cx
        add bx, 44
        inc word [cur_idx]
        dec cx
        jnz .ap_loop
        jmp .done

.absent:
        mov si, msg_absent
        call print_str

.done:
        mov ax, 0x4C00
        int 0x21

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

print_dec32:
        push eax
        push ebx
        push cx
        push edx
        mov ebx, 10
        xor cx, cx
.div:
        xor edx, edx
        div ebx
        push dx
        inc cx
        test eax, eax
        jnz .div
.out:
        pop dx
        add dl, '0'
        mov al, dl
        call print_char
        loop .out
        pop edx
        pop cx
        pop ebx
        pop eax
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

jlm_off:    dw 0
jlm_seg:    dw 0
dma_seg:    dw 0
cur_idx:    dw 0

msg_title:  db 'WSCAN: Ferric DOS Atheros AR2413 2.4 GHz beacon scan (Gate 4b.4b)', 13, 10, '$'
msg_no_jlm: db 'WSCAN: ERROR FERRIC.JLM not loaded (boot Profile 1 or 2)', 13, 10, '$'
msg_no_mem: db 'WSCAN: ERROR 16 KiB DMA alloc failed', 13, 10, '$'
msg_absent: db 'WSCAN: present=0 stage=0 aps=0', 13, 10, '$'
msg_sum1:   db 'WSCAN: present=1 stage=$'
msg_rx:     db ' rx=$'
msg_ok:     db ' ok=$'
msg_bcn:    db ' beacons=$'
msg_aps:    db ' aps=$'
msg_ap:     db 'WSCAN: ap=$'
msg_ch:     db ' ch=$'
msg_rssi:   db ' rssi=$'
msg_sec:    db ' sec=$'
msg_wpa2:   db 'WPA2$'
msg_wep:    db 'PRIV$'
msg_open:   db 'OPEN$'
msg_bssid:  db ' bssid=$'
msg_ssid:   db ' ssid=$'
msg_hidden: db '<hidden>$'
msg_crlf:   db 13, 10, '$'

scan_buf:   times 720 db 0
wscan_end:
