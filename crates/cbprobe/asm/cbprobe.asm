; CBPROBE.COM - Gate 4b step 4b.1: read-only CardBus bridge probe.
;
; Dumps the PCI configuration space of the two Toshiba ToPIC95
; CardBus bridges (1179:0617 at 00:11.0 / 00:11.1) and decodes the
; fields the Gate 4b bridge driver will program: BAR0, bus numbers,
; command/status, interrupt line/pin, bridge control (incl. the
; WBEN write-buffer bit), and the Toshiba config registers (socket
; control 0x90, slot control 0xA0, interrupt control 0xA1, card
; detect 0xA3, register control 0xA4; layouts from Linux topic.h,
; vendored under vendor/ref/linux-ath/).
;
; Read-only by construction: the only port writes in this program
; are the PCI configuration address writes to 0xCF8 that every
; config read requires. No config data write (0xCFC store) exists
; in the code, and no other device is touched.
;
; Output is one summary line per function plus a full 256-byte
; config dump, all prefixed CBPROBE: so a capture greps cleanly.
; Absence is a report, not an error: exit code is always 0.

        org 0x100
        bits 16

start:
        mov si, msg_title
        call print_str

        mov byte [bridges], 0
        mov byte [cur_fn], 0
        call probe_fn
        mov byte [cur_fn], 1
        call probe_fn

        mov si, msg_bridges
        call print_str
        mov al, [bridges]
        xor ah, ah
        call print_dec16
        call print_crlf

        mov ax, 0x4C00
        int 0x21

; probe_fn: dump the bridge at device 17, function [cur_fn].
probe_fn:
        ; cfg_base = 0x80000000 | (17 << 11) | (fn << 8)
        mov eax, 0x80000000
        mov ecx, 17
        shl ecx, 11
        or eax, ecx
        movzx ecx, byte [cur_fn]
        shl ecx, 8
        or eax, ecx
        mov [cfg_base], eax

        xor al, al
        call pci_read32         ; EAX = vendor | device << 16
        mov [id_dword], eax
        mov ax, [id_dword]      ; vendor id (low word)
        cmp ax, 0xFFFF
        je .absent
        or ax, ax
        jz .absent
        inc byte [bridges]

        ; --- summary line ---
        mov si, msg_fn
        call print_str
        mov al, [cur_fn]
        xor ah, ah
        call print_dec16
        mov si, msg_present1
        call print_str
        mov si, msg_id
        call print_str
        mov ax, [id_dword]
        call print_hex16
        mov al, ':'
        call print_char
        mov eax, [id_dword]
        shr eax, 16
        call print_hex16

        mov al, 0x08
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_rev
        call print_str
        mov al, [tmp_dword]
        call print_hex8
        mov si, msg_class
        call print_str
        mov eax, [tmp_dword]
        shr eax, 8              ; class code, 24 bits
        call print_hex24

        mov al, 0x0C
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_hdr
        call print_str
        mov al, [tmp_dword + 2] ; header type at 0x0E
        call print_hex8

        mov al, 0x04
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_cmd
        call print_str
        mov ax, [tmp_dword]
        call print_hex16
        mov si, msg_stat
        call print_str
        mov eax, [tmp_dword]
        shr eax, 16
        call print_hex16

        mov al, 0x10
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_bar0
        call print_str
        mov eax, [tmp_dword]
        call print_hex32

        mov al, 0x14
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_bus
        call print_str
        mov al, [tmp_dword]     ; primary
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [tmp_dword + 1] ; secondary
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [tmp_dword + 2] ; subordinate
        call print_hex8

        mov al, 0x3C
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_intline
        call print_str
        mov al, [tmp_dword]
        call print_hex8
        mov si, msg_intpin
        call print_str
        mov al, [tmp_dword + 1]
        call print_hex8
        mov si, msg_ppbcn
        call print_str
        mov ax, [tmp_dword + 2] ; bridge control at 0x3E
        mov [ppbcn], ax
        call print_hex16
        mov si, msg_wben
        call print_str
        mov ax, [ppbcn]
        and ax, 0x0400
        jz .wben0
        mov al, '1'
        jmp .wben_done
.wben0:
        mov al, '0'
.wben_done:
        call print_char

        mov al, 0x90
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_scr
        call print_str
        mov eax, [tmp_dword]
        call print_hex32

        mov al, 0xA0
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_slotctl
        call print_str
        mov al, [tmp_dword]     ; slot control 0xA0
        call print_hex8
        mov si, msg_intctl
        call print_str
        mov al, [tmp_dword + 1] ; interrupt control 0xA1
        call print_hex8
        mov si, msg_cardet
        call print_str
        mov al, [tmp_dword + 3] ; card detect 0xA3
        mov [cardet], al
        call print_hex8
        mov si, msg_pc32
        call print_str
        mov al, [cardet]
        and al, 0x80
        jz .pc32_0
        mov al, '1'
        jmp .pc32_done
.pc32_0:
        mov al, '0'
.pc32_done:
        call print_char
        mov si, msg_vs
        call print_str
        mov al, [cardet]
        and al, 0x04
        jz .vs1_0
        mov al, '1'
        jmp .vs1_done
.vs1_0:
        mov al, '0'
.vs1_done:
        call print_char
        mov al, '/'
        call print_char
        mov al, [cardet]
        and al, 0x02
        jz .vs2_0
        mov al, '1'
        jmp .vs2_done
.vs2_0:
        mov al, '0'
.vs2_done:
        call print_char
        mov si, msg_swdet
        call print_str
        mov al, [cardet]
        and al, 0x01
        jz .sw_0
        mov al, '1'
        jmp .sw_done
.sw_0:
        mov al, '0'
.sw_done:
        call print_char

        mov al, 0xA4
        call pci_read32
        mov [tmp_dword], eax
        mov si, msg_rcr
        call print_str
        mov eax, [tmp_dword]
        call print_hex32
        call print_crlf

        ; --- full config dump, 16 lines of 16 bytes ---
        mov byte [dump_off], 0
.dump_line:
        mov si, msg_dump
        call print_str
        mov al, [cur_fn]
        xor ah, ah
        call print_dec16
        mov si, msg_dump_off
        call print_str
        mov al, [dump_off]
        call print_hex8
        mov al, ':'
        call print_char
        mov byte [dump_col], 0
.dump_byte:
        mov al, ' '
        call print_char
        mov al, [dump_off]
        add al, [dump_col]
        call pci_read32         ; dword containing the byte
        mov [tmp_dword], eax
        movzx eax, byte [dump_col]
        and eax, 3
        ; byte index within the dword, little-endian order
        mov ecx, eax
        mov eax, [tmp_dword]
        jecxz .shifted
.shift:
        shr eax, 8
        loop .shift
.shifted:
        call print_hex8         ; AL = the byte
        inc byte [dump_col]
        cmp byte [dump_col], 16
        jb .dump_byte
        call print_crlf
        add byte [dump_off], 16
        jnz .dump_line
        ret

.absent:
        mov si, msg_fn
        call print_str
        mov al, [cur_fn]
        xor ah, ah
        call print_dec16
        mov si, msg_present0
        call print_str
        call print_crlf
        ret

; pci_read32: AL = config offset (dword-aligned by caller usage;
; any offset works, the containing dword is returned).
; Returns EAX = the config dword. Clobbers EDX.
pci_read32:
        push ecx
        movzx ecx, al
        and ecx, 0xFC
        mov eax, [cfg_base]
        or eax, ecx
        mov dx, 0xCF8
        out dx, eax
        mov dx, 0xCFC
        in eax, dx
        pop ecx
        ret

; --- print helpers (APMSTAT idiom) ---

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

; print_hex8: AL.
print_hex8:
        push ax
        push bx
        mov bl, al
        shr al, 4
        call print_nibble
        mov al, bl
        and al, 0x0F
        call print_nibble
        pop bx
        pop ax
        ret

; print_hex16: AX.
print_hex16:
        push ax
        mov al, ah
        call print_hex8
        pop ax
        call print_hex8
        ret

; print_hex24: EAX (low 24 bits).
print_hex24:
        push eax
        shr eax, 16
        call print_hex8
        pop eax
        call print_hex16
        ret

; print_hex32: EAX.
print_hex32:
        push eax
        shr eax, 16
        call print_hex16
        pop eax
        call print_hex16
        ret

print_nibble:
        and al, 0x0F
        cmp al, 10
        jb .digit
        add al, 'A' - 10
        jmp print_char
.digit:
        add al, '0'
        jmp print_char

; --- data ---

cfg_base:   dd 0
id_dword:   dd 0
tmp_dword:  dd 0
ppbcn:      dw 0
cardet:     db 0
bridges:    db 0
cur_fn:     db 0
dump_off:   db 0
dump_col:   db 0

msg_title:   db 'Ferric DOS CardBus bridge probe', 0x0D, 0x0A, '$'
msg_fn:      db 'CBPROBE: fn=$'
msg_present1: db ' present=1$'
msg_present0: db ' present=0$'
msg_id:      db ' id=$'
msg_rev:     db ' rev=$'
msg_class:   db ' class=$'
msg_hdr:     db ' hdr=$'
msg_cmd:     db ' cmd=$'
msg_stat:    db ' stat=$'
msg_bar0:    db ' bar0=$'
msg_bus:     db ' bus=$'
msg_intline: db ' intline=$'
msg_intpin:  db ' intpin=$'
msg_ppbcn:   db ' ppbcn=$'
msg_wben:    db ' wben=$'
msg_scr:     db ' scr=$'
msg_slotctl: db ' slotctl=$'
msg_intctl:  db ' intctl=$'
msg_cardet:  db ' cardet=$'
msg_pc32:    db ' pc32=$'
msg_vs:      db ' vs=$'
msg_swdet:   db ' swdet=$'
msg_rcr:     db ' rcr=$'
msg_dump:    db 'CBPROBE: cfg fn=$'
msg_dump_off: db ' off=$'
msg_bridges: db 'CBPROBE: bridges=$'
msg_crlf:    db 0x0D, 0x0A, '$'
