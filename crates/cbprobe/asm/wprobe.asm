; =============================================================================
; WPROBE.COM - Gate 4b step 4b.4a: Atheros AR2413 MAC/EEPROM bring-up probe
; =============================================================================
; Calls FERRIC.JLM V86 API AX=0x000A (gate1_wifi_probe) to locate an Atheros
; CardBus NIC (168C:xxxx, auto-running CardBus bridge bring-up if needed),
; map BAR0 uncached in Ring 0, wake/reset the MAC, read SREV + PHY ID +
; EEPROM magic/version/header/misc5/regdomain/MAC, and check hardware AES-CCM
; support (ath5k attach.c:315).
;
; Strictly 386 baseline, read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

start:
        push cs
        pop ds
        push cs
        pop es

        ; Check PSP command tail (0x80) for mode flag:
        ;   /D -> diag_mode=1 (non-destructive Wi-Fi diag dump)
        ;   /E -> diag_mode=2 (switch INT 60h -> Ethernet)
        ;   /W -> diag_mode=3 (switch INT 60h -> Wi-Fi)
        ;   /X -> diag_mode=4 (disconnect Wi-Fi & power down CardBus)
        mov byte [diag_mode], 0
        movzx cx, byte [0x0080]
        test cx, cx
        jz .no_args
        mov si, 0x0081
.arg_scan:
        lodsb
        cmp al, 'D'
        je .set_diag
        cmp al, 'd'
        je .set_diag
        cmp al, 'E'
        je .set_eth
        cmp al, 'e'
        je .set_eth
        cmp al, 'W'
        je .set_wifi
        cmp al, 'w'
        je .set_wifi
        cmp al, 'X'
        je .set_unload
        cmp al, 'x'
        je .set_unload
        loop .arg_scan
        jmp .no_args
.set_diag:
        mov byte [diag_mode], 1
        jmp .no_args
.set_eth:
        mov byte [diag_mode], 2
        jmp .no_args
.set_wifi:
        mov byte [diag_mode], 3
        jmp .no_args
.set_unload:
        mov byte [diag_mode], 4
.no_args:
        cmp byte [diag_mode], 0
        jne .find_jlm
        mov si, msg_title
        call print_str

.find_jlm:
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
        cmp byte [diag_mode], 0
        je .do_probe
        cmp byte [diag_mode], 2
        je .do_eth
        cmp byte [diag_mode], 3
        je .do_wifi
        cmp byte [diag_mode], 4
        je .do_unload

        ; Non-destructive live Wi-Fi diag dump (AX=0x000D).
        push cs
        pop es
        mov di, diag_buf
        mov cx, 1536
        mov ax, 0x000D
        call far [jlm_off]
        mov cx, ax
        test cx, cx
        jz .done
        mov si, diag_buf
.diag_print:
        lodsb
        call print_char
        loop .diag_print
        jmp .done

.do_eth:
        xor bx, bx
        mov ax, 0x000E
        call far [jlm_off]
        test ax, ax
        jz .eth_fail
        mov si, msg_sw_eth
        call print_str
        jmp .done
.eth_fail:
        mov si, msg_sw_eth_fail
        call print_str
        jmp .done

.do_wifi:
        mov bx, 1
        mov ax, 0x000E
        call far [jlm_off]
        test ax, ax
        jz .wifi_fail
        mov si, msg_sw_wifi
        call print_str
        jmp .done
.wifi_fail:
        mov si, msg_sw_wifi_fail
        call print_str
        jmp .done

.do_unload:
        mov ax, 0x000F
        call far [jlm_off]
        mov si, msg_unloaded
        call print_str
        jmp .done

.do_probe:
        push cs
        pop es
        mov di, probe_buf
        mov ax, 0x000A
        call far [jlm_off]

        cmp byte [probe_buf + 0x00], 0
        je .absent

        ; Line 1: WPROBE: present=1 stage=S slot=N id=VVVV:DDDD bar0=0xXXXXXXXX
        mov si, msg_p1
        call print_str
        movzx ax, byte [probe_buf + 0x01]
        call print_dec16
        mov si, msg_slot
        call print_str
        movzx ax, byte [probe_buf + 0x02]
        call print_dec16
        mov si, msg_id
        call print_str
        mov ax, [probe_buf + 0x04]
        call print_hex16
        mov al, ':'
        call print_char
        mov ax, [probe_buf + 0x06]
        call print_hex16
        mov si, msg_bar0
        call print_str
        mov eax, [probe_buf + 0x08]
        call print_hex32
        call print_crlf

        ; Line 2: WPROBE: srev=0xXXXXXXXX phy=0xXXXXXXXX
        mov si, msg_srev
        call print_str
        mov eax, [probe_buf + 0x10]
        call print_hex32
        mov si, msg_phy
        call print_str
        mov eax, [probe_buf + 0x14]
        call print_hex32
        call print_crlf

        ; If stage < 4, stop here
        cmp byte [probe_buf + 0x01], 4
        jb .done

        ; Line 3: WPROBE: ee_magic=0xXXXX ee_ver=0xXXXX ee_hdr=0xXXXX misc5=0xXXXX regdom=0xXXXX
        mov si, msg_ee
        call print_str
        mov ax, [probe_buf + 0x18]
        call print_hex16
        mov si, msg_ever
        call print_str
        mov ax, [probe_buf + 0x1A]
        call print_hex16
        mov si, msg_ehdr
        call print_str
        mov ax, [probe_buf + 0x1C]
        call print_hex16
        mov si, msg_misc5
        call print_str
        mov ax, [probe_buf + 0x1E]
        call print_hex16
        mov si, msg_regdom
        call print_str
        mov ax, [probe_buf + 0x20]
        call print_hex16
        call print_crlf

        ; Line 4: WPROBE: mac=XX-XX-XX-XX-XX-XX mac_ok=M aes_hw=A
        mov si, msg_mac
        call print_str
        mov si, probe_buf + 0x24
        mov cx, 6
.mac_loop:
        mov al, [si]
        call print_hex8
        inc si
        dec cx
        jz .mac_done
        mov al, '-'
        call print_char
        jmp .mac_loop
.mac_done:
        mov si, msg_macok
        call print_str
        movzx ax, byte [probe_buf + 0x22]
        call print_dec16
        mov si, msg_aes
        call print_str
        movzx ax, byte [probe_buf + 0x03]
        call print_dec16
        call print_crlf
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

print_hex16:
        push ax
        xchg ah, al
        call print_hex8
        xchg ah, al
        call print_hex8
        pop ax
        ret

print_hex32:
        push eax
        shr eax, 16
        call print_hex16
        pop eax
        call print_hex16
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
diag_mode:  db 0
probe_buf:  times 44 db 0

msg_title:  db 'WPROBE: Ferric DOS Atheros AR2413 MAC/EEPROM probe (Gate 4b.4a)', 13, 10, '$'
msg_no_jlm: db 'WPROBE: ERROR FERRIC.JLM not loaded (boot Profile 1 or 2)', 13, 10, '$'
msg_absent: db 'WPROBE: present=0 stage=0', 13, 10, '$'
msg_p1:     db 'WPROBE: present=1 stage=$'
msg_slot:   db ' slot=$'
msg_id:     db ' id=$'
msg_bar0:   db ' bar0=0x$'
msg_srev:   db 'WPROBE: srev=0x$'
msg_phy:    db ' phy=0x$'
msg_ee:     db 'WPROBE: ee_magic=0x$'
msg_ever:   db ' ee_ver=0x$'
msg_ehdr:   db ' ee_hdr=0x$'
msg_misc5:  db ' misc5=0x$'
msg_regdom: db ' regdom=0x$'
msg_mac:    db 'WPROBE: mac=$'
msg_macok:  db ' mac_ok=$'
msg_aes:    db ' aes_hw=$'
msg_sw_eth: db 'WPROBE: INT 60h -> Ethernet (82550)', 13, 10, '$'
msg_sw_eth_fail: db 'WPROBE: Ethernet not initialized', 13, 10, '$'
msg_sw_wifi: db 'WPROBE: INT 60h -> Wi-Fi (AR2413)', 13, 10, '$'
msg_sw_wifi_fail: db 'WPROBE: Wi-Fi not connected', 13, 10, '$'
msg_unloaded: db 'WPROBE: Wi-Fi disconnected & CardBus powered down', 13, 10, '$'
msg_crlf:   db 13, 10, '$'

diag_buf    equ $
