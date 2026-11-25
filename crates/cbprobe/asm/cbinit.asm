; =============================================================================
; CBINIT.COM - Gate 4b step 4b.3: Toshiba ToPIC95 CardBus bridge bring-up
; =============================================================================
; Calls FERRIC.JLM V86 API AX=0x0008 (gate1_cb_init) and AX=0x0009
; (gate1_cb_slot_info) to configure both ToPIC95 CardBus bridges (00:11.0 /
; 00:11.1), power any inserted 3.3 V CardBus card (D-Link DWL-G630 C2 /
; Atheros AR2413), release CardBus reset, enumerate the card on the secondary
; bus, and report all observed IDs/BARs with CBINIT: markers.
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
        push cs
        pop es

        ; AX=0x0008 -> AX=bridges, BX=cards
        mov ax, 0x0008
        call far [jlm_off]
        mov [bridges], ax
        mov [cards], bx

        ; Query and print slot 0 and slot 1
        mov word [cur_slot], 0
        call dump_slot
        mov word [cur_slot], 1
        call dump_slot

        ; Summary line
        mov si, msg_sum_b
        call print_str
        mov ax, [bridges]
        call print_dec16
        mov si, msg_sum_c
        call print_str
        mov ax, [cards]
        call print_dec16
        call print_crlf

        mov ax, 0x4C00
        int 0x21

dump_slot:
        push cs
        pop es
        mov di, slot_buf
        mov bx, [cur_slot]
        mov ax, 0x0009
        call far [jlm_off]
        cmp ax, 1
        jne .done

        cmp byte [slot_buf + 0x00], 0
        je .absent

        ; Present bridge line:
        ; CBINIT: slot=N stage=S rev=0xRR bus=B bar0=0xXXXXXXXX mem0=0xXXXXXXXX state=0xXXXXXXXX card=C cb32=C pwr3v=P
        mov si, msg_slot
        call print_str
        mov ax, [cur_slot]
        call print_dec16
        mov si, msg_stage
        call print_str
        movzx ax, byte [slot_buf + 0x01]
        call print_dec16
        mov si, msg_rev
        call print_str
        mov al, [slot_buf + 0x02]
        call print_hex8
        mov si, msg_bus
        call print_str
        movzx ax, byte [slot_buf + 0x03]
        call print_dec16
        mov si, msg_bar0
        call print_str
        mov eax, [slot_buf + 0x04]
        call print_hex32
        mov si, msg_mem0
        call print_str
        mov eax, [slot_buf + 0x08]
        call print_hex32
        mov si, msg_state
        call print_str
        mov eax, [slot_buf + 0x0C]
        call print_hex32
        mov si, msg_card
        call print_str
        movzx ax, byte [slot_buf + 0x10]
        call print_dec16
        mov si, msg_cb32
        call print_str
        movzx ax, byte [slot_buf + 0x11]
        call print_dec16
        mov si, msg_pwr
        call print_str
        movzx ax, byte [slot_buf + 0x12]
        call print_dec16
        call print_crlf

        ; If stage >= 5, also print enumerated CardBus card line
        cmp byte [slot_buf + 0x01], 5
        jb .done

        mov si, msg_cslot
        call print_str
        mov ax, [cur_slot]
        call print_dec16
        mov si, msg_cid
        call print_str
        mov ax, [slot_buf + 0x14]
        call print_hex16
        mov al, ':'
        call print_char
        mov ax, [slot_buf + 0x16]
        call print_hex16
        mov si, msg_rev
        call print_str
        mov al, [slot_buf + 0x13]
        call print_hex8
        mov si, msg_class
        call print_str
        mov eax, [slot_buf + 0x1C]
        call print_hex24
        mov si, msg_subsys
        call print_str
        mov ax, [slot_buf + 0x18]
        call print_hex16
        mov al, ':'
        call print_char
        mov ax, [slot_buf + 0x1A]
        call print_hex16
        mov si, msg_bar0
        call print_str
        mov eax, [slot_buf + 0x20]
        call print_hex32
        call print_crlf
        ret

.absent:
        mov si, msg_slot
        call print_str
        mov ax, [cur_slot]
        call print_dec16
        mov si, msg_absent
        call print_str
        call print_crlf
.done:
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

print_hex24:
        push eax
        shr eax, 16
        call print_hex8
        pop eax
        call print_hex16
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
bridges:    dw 0
cards:      dw 0
cur_slot:   dw 0
slot_buf:   times 40 db 0

msg_title:  db 'CBINIT: Ferric DOS ToPIC95 CardBus bridge bring-up (Gate 4b.3)', 13, 10, '$'
msg_no_jlm: db 'CBINIT: ERROR FERRIC.JLM not loaded (boot Profile 1 or 2)', 13, 10, '$'
msg_slot:   db 'CBINIT: slot=$'
msg_absent: db ' present=0$'
msg_stage:  db ' stage=$'
msg_rev:    db ' rev=0x$'
msg_bus:    db ' bus=$'
msg_bar0:   db ' bar0=0x$'
msg_mem0:   db ' mem0=0x$'
msg_state:  db ' state=0x$'
msg_card:   db ' card=$'
msg_cb32:   db ' cb32=$'
msg_pwr:    db ' pwr3v=$'
msg_cslot:  db 'CBINIT: card slot=$'
msg_cid:    db ' id=$'
msg_class:  db ' class=0x$'
msg_subsys: db ' subsys=$'
msg_sum_b:  db 'CBINIT: bridges=$'
msg_sum_c:  db ' cards=$'
msg_crlf:   db 13, 10, '$'
