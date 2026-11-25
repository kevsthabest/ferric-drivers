; NTEST.COM - Gate 2 step 2.3 packet-driver test tool.
;
; Exercises the resident Ferric packet driver exactly the way a real
; client finds and uses it, no Ferric-only shortcuts in the path that
; matters:
;   1. scan vectors 60h-7Fh for the "PKT DRVR" signature,
;   2. driver_info + the Ferric diagnostic (stage / I/O / IRQ / link),
;   3. get_address,
;   4. access_type (accept-all) with a two-phase receiver,
;   5. send an ARP request for the QEMU slirp gateway (10.0.2.2) and
;      wait for the reply to arrive through the receiver upcall,
;   6. get_statistics, release_type.
;
; Verdict lines (the QEMU harness asserts on them):
;   NTEST:SUMMARY found=1 mac_ok=1 arp=1
;   NTEST: PASS

bits 16
org 0x100

start:
    ; ---- 1. find the driver by signature
    mov byte [vec], 0x60
.scan:
    mov ah, 0x35
    mov al, [vec]
    int 0x21                    ; ES:BX = handler
    push ds
    push es
    pop ds
    mov si, bx
    add si, 3                   ; signature lives at handler+3
    push cs
    pop es
    mov di, sig_str
    mov cx, 8
    repe cmpsb
    pop ds
    je .found
    inc byte [vec]
    cmp byte [vec], 0x80
    jb .scan
    mov si, msg_found0
    call print_str
    call print_summary
    mov si, msg_fail
    call print_str
    mov ax, 0x4C01
    int 0x21
.found:
    mov al, [vec]
    mov [pkt_vec], al
    mov byte [found_v], 1
    mov si, msg_found
    call print_str
    mov al, [vec]
    call print_hex8
    call print_crlf

    ; ---- 2a. Ferric diagnostic (AH=80h): stage / io / irq / link
    mov ah, 0x80
    call pkt_call
    jc .diag_done
    mov [diag_stage], ax
    mov [diag_io], bx
    mov [diag_cx], cx
    mov [diag_dx], dx
    mov si, msg_stage
    call print_str
    mov ax, [diag_stage]
    call print_dec16
    mov si, msg_io
    call print_str
    mov ax, [diag_io]
    call print_hex16
    mov si, msg_irq
    call print_str
    mov ax, [diag_cx]
    and ax, 0xFF
    call print_dec16
    mov si, msg_link
    call print_str
    mov ax, [diag_cx]
    mov al, ah
    and ax, 1
    call print_dec16
    mov si, msg_macsrc
    call print_str
    mov ax, [diag_cx]
    mov al, ah
    shr al, 1
    and ax, 1
    call print_dec16
    mov si, msg_irqcnt
    call print_str
    mov ax, [diag_dx]
    call print_dec16
    call print_crlf
.diag_done:

    ; ---- 2b. driver_info (AH=01h, AL=FFh)
    mov ax, 0x01FF
    call pkt_call
    jc .info_fail
    push bx
    push cx
    ; (results ride the stack across the name copy: this call returns
    ; with DS = the stub segment, so storing to our data now would
    ; write into the stub block)
    ; DS:SI = name in the stub block; copy it before DS is ours again.
    push es
    push cs
    pop es
    mov di, name_buf
    mov cx, 47
.name_loop:
    lodsb
    or al, al
    jz .name_done
    stosb
    loop .name_loop
.name_done:
    mov al, '$'
    stosb
    pop es
    push cs
    pop ds
    pop cx
    pop bx
    mov [info_ver], bx
    mov [info_class], ch
    mov si, msg_ver
    call print_str
    mov ax, [info_ver]
    call print_hex16
    mov si, msg_class
    call print_str
    mov al, [info_class]
    xor ah, ah
    call print_dec16
    mov si, msg_name
    call print_str
    mov si, name_buf
    call print_str
    call print_crlf
    jmp .info_done
.info_fail:
    mov si, msg_infofail
    call print_str
.info_done:

    ; ---- 3. get_address (BX=0 pre-registration is allowed)
    push cs
    pop es
    mov di, mac_buf
    mov cx, 6
    xor bx, bx
    mov ah, 0x06
    call pkt_call
    jc .addr_fail
    mov si, msg_mac
    call print_str
    xor bx, bx
.mac_loop:
    mov al, [mac_buf + bx]
    call print_hex8
    inc bx
    cmp bx, 6
    jae .mac_done
    mov al, '-'
    call print_char
    jmp .mac_loop
.mac_done:
    call print_crlf
    ; mac_ok = neither all-zero nor all-FF
    mov byte [mac_ok_v], 1
    xor bx, bx
    mov al, 0
.mac_or:
    or al, [mac_buf + bx]
    inc bx
    cmp bx, 6
    jb .mac_or
    cmp al, 0
    je .mac_bad
    xor bx, bx
    mov al, 0xFF
.mac_and:
    and al, [mac_buf + bx]
    inc bx
    cmp bx, 6
    jb .mac_and
    cmp al, 0xFF
    jne .addr_done
.mac_bad:
    mov byte [mac_ok_v], 0
    jmp .addr_done
.addr_fail:
    mov si, msg_addrfail
    call print_str
    mov byte [mac_ok_v], 0
.addr_done:

    ; ---- 4. access_type: class 1, any type, typelen 0 (accept all)
    push cs
    pop es
    mov di, receiver
    mov cx, 0
    mov bx, 0xFFFF
    mov dl, 0
    mov ax, 0x0201
    call pkt_call
    jc .access_fail
    mov [handle], ax
    jmp .access_done
.access_fail:
    mov si, msg_accfail
    call print_str
    jmp .finish
.access_done:

    ; ---- 5. ARP request for 10.0.2.2, from 10.0.2.15
    mov di, tx_buf
    mov cx, 6
    mov al, 0xFF
    rep stosb                   ; destination: broadcast
    push cs
    pop ds
    mov si, mac_buf
    mov cx, 6
    rep movsb                   ; source: our MAC
    mov word [di], 0x0608       ; EtherType ARP (wire order)
    add di, 2
    mov word [di], 0x0100       ; HTYPE Ethernet
    mov word [di+2], 0x0008     ; PTYPE IPv4
    mov byte [di+4], 6          ; HLEN
    mov byte [di+5], 4          ; PLEN
    mov word [di+6], 0x0100     ; OPER request
    add di, 8
    mov si, mac_buf
    mov cx, 6
    rep movsb                   ; sender hardware address
    mov dword [di], 0x0F02000A  ; sender protocol address 10.0.2.15
    add di, 4
    mov cx, 6
    xor al, al
    rep stosb                   ; target hardware address: unknown
    mov dword [di], 0x0202000A  ; target protocol address 10.0.2.2
    mov si, tx_buf
    mov cx, 42
    mov ah, 0x04
    call pkt_call
    jc .send_fail
    jmp .wait
.send_fail:
    mov si, msg_sendfail
    call print_str
    jmp .release
.wait:
    call read_tick
    mov [start_tick], eax
.wait_loop:
    cmp byte [arp_flag], 0
    jne .got_arp
    call read_tick
    sub eax, [start_tick]
    cmp eax, 200                ; ~11 s
    jb .wait_loop
    jmp .release
.got_arp:
    mov byte [arp_v], 1
    mov si, msg_arp
    call print_str
    xor bx, bx
.sha_loop:
    mov al, [arp_sha + bx]
    call print_hex8
    inc bx
    cmp bx, 6
    jae .release
    mov al, '-'
    call print_char
    jmp .sha_loop

    ; ---- 6. statistics + release
.release:
    push cs
    pop ds
    mov si, stats_buf
    mov bx, [handle]
    mov ah, 0x18
    call pkt_call
    jc .stats_fail
    jmp .stats_ok
.stats_fail:
    push dx
    mov si, msg_statsfail
    call print_str
    pop dx
    mov al, dh
    xor ah, ah
    call print_dec16
    call print_crlf
    jmp .rel_now
.stats_ok:
    mov si, msg_stats
    call print_str
    mov eax, [stats_buf]
    call print_dec32
    mov si, msg_stats_tx
    call print_str
    mov eax, [stats_buf + 4]
    call print_dec32
    call print_crlf
.rel_now:
    mov bx, [handle]
    mov ah, 0x03
    call pkt_call

.finish:
    call print_summary
    cmp byte [mac_ok_v], 1
    jne .fail
    cmp byte [arp_v], 1
    jne .fail
    mov si, msg_pass
    call print_str
    mov ax, 0x4C00
    int 0x21
.fail:
    mov si, msg_fail
    call print_str
    mov ax, 0x4C01
    int 0x21

print_summary:
    mov si, msg_summary
    call print_str
    mov al, [found_v]
    xor ah, ah
    call print_dec16
    mov si, msg_s_mac
    call print_str
    mov al, [mac_ok_v]
    xor ah, ah
    call print_dec16
    mov si, msg_s_arp
    call print_str
    mov al, [arp_v]
    xor ah, ah
    call print_dec16
    call print_crlf
    ret

; Packet-driver invocation through the discovered vector.
pkt_call:
    db 0xCD
pkt_vec:
    db 0x60
    ret

; Two-phase receiver (far-called by the driver via nested execution).
; Phase 0 (AX=0): offer our frame buffer in ES:DI.
; Phase 1 (AX=1): DS:SI points at the copied frame; recognize the
; slirp gateway's ARP reply and remember its sender address.
receiver:
    cmp ax, 0
    je .phase0
    cmp word [si+12], 0x0608    ; EtherType ARP
    jne .out
    cmp word [si+20], 0x0200    ; OPER = reply
    jne .out
    cmp dword [si+28], 0x0202000A ; sender protocol = 10.0.2.2
    jne .out
    push si
    push di
    push cx
    push es
    push cs
    pop es
    mov di, arp_sha
    add si, 22                  ; sender hardware address
    mov cx, 6
    rep movsb
    pop es
    pop cx
    pop di
    pop si
    mov byte [cs:arp_flag], 1
.out:
    retf
.phase0:
    push cs
    pop es
    mov di, rx_buf
    retf

read_tick:
    push ds
    xor ax, ax
    mov ds, ax
    mov eax, [0x046C]
    pop ds
    ret

print_hex16:
    push ax
    mov al, ah
    call print_hex8
    pop ax
    call print_hex8
    ret

print_hex8:
    push ax
    push cx
    mov cl, 4
    shr al, cl
    call .nib
    pop cx
    pop ax
    push ax
    call .nib
    pop ax
    ret
.nib:
    and al, 0x0F
    cmp al, 10
    jb .dig
    add al, 'A' - 10
    jmp print_char
.dig:
    add al, '0'
    jmp print_char

print_dec32:
    push bx
    push cx
    push dx
    mov ebx, 10
    xor cx, cx
.d32:
    xor edx, edx
    div ebx
    push dx
    inc cx
    or eax, eax
    jnz .d32
.o32:
    pop ax
    add al, '0'
    call print_char
    loop .o32
    pop dx
    pop cx
    pop bx
    ret

print_dec16:
    push bx
    push cx
    push dx
    mov bx, 10
    xor cx, cx
.div_loop:
    xor dx, dx
    div bx
    push dx
    inc cx
    or ax, ax
    jnz .div_loop
.out_loop:
    pop ax
    add al, '0'
    call print_char
    loop .out_loop
    pop dx
    pop cx
    pop bx
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

print_str:
    push ax
    mov ah, 0x09
    mov dx, si
    int 0x21
    pop ax
    ret

print_crlf:
    mov si, msg_crlf
    call print_str
    ret

vec:        db 0x60
found_v:    db 0
mac_ok_v:   db 0
arp_v:      db 0
arp_flag:   db 0
handle:     dw 0
diag_stage: dw 0
diag_io:    dw 0
diag_cx:    dw 0
diag_dx:    dw 0
info_ver:   dw 0
info_class: db 0
start_tick: dd 0
mac_buf:    times 6 db 0
arp_sha:    times 6 db 0
name_buf:   times 50 db 0
stats_buf:  times 28 db 0
tx_buf:     times 64 db 0
rx_buf:     times 1536 db 0

sig_str:    db 'PKT DRVR', 0

msg_found:   db 'NTEST: found=1 int=$'
msg_found0:  db 'NTEST: found=0', 0x0D, 0x0A, '$'
msg_stage:   db 'NTEST: stage=$'
msg_io:      db ' io=$'
msg_irq:     db ' irq=$'
msg_link:    db ' link=$'
msg_macsrc:  db ' macsrc=$'
msg_irqcnt:  db ' irqcnt=$'
msg_ver:     db 'NTEST: ver=$'
msg_class:   db ' class=$'
msg_name:    db ' name=$'
msg_infofail: db 'NTEST: driver_info FAILED', 0x0D, 0x0A, '$'
msg_mac:     db 'NTEST: mac=$'
msg_addrfail: db 'NTEST: get_address FAILED', 0x0D, 0x0A, '$'
msg_accfail: db 'NTEST: access_type FAILED', 0x0D, 0x0A, '$'
msg_sendfail: db 'NTEST: send_pkt FAILED', 0x0D, 0x0A, '$'
msg_arp:     db 'NTEST: arp_sha=$'
msg_statsfail: db 'NTEST: stats FAILED dh=$'
msg_stats:   db 'NTEST: stats rx=$'
msg_stats_tx: db ' tx=$'
msg_summary: db 'NTEST:SUMMARY found=$'
msg_s_mac:   db ' mac_ok=$'
msg_s_arp:   db ' arp=$'
msg_pass:    db 'NTEST: PASS', 0x0D, 0x0A, '$'
msg_fail:    db 'NTEST: FAIL', 0x0D, 0x0A, '$'
msg_crlf:    db 0x0D, 0x0A, '$'
