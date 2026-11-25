; =============================================================================
; VBEPROBE.COM - Stage G Step 1: Read-Only VESA VBE 2.0/3.0 Recon Utility
; =============================================================================
; Strictly read-only (never switches video modes):
;   - INT 10h AX=4F00h ("VBE2"): VBE version, OEM string, VRAM size, caps
;   - INT 10h AX=4F03h: current VBE mode
;   - INT 10h AX=4F0Ah: VBE 2.0 protected-mode interface presence
;   - INT 10h AX=4F01h: walks VideoModePtr and prints graphics modes
;     (mode hex, WxHxBPP, pitch, LFB/BANK flags, PhysBasePtr)
;
; Strictly 386 baseline, 16-bit real-mode COM.
; =============================================================================

cpu 386
bits 16
org 0x0100

start:
        push cs
        pop ds
        push cs
        pop es

        mov si, msg_banner
        call print_str

        ; Prepare 512-byte VbeInfoBlock with "VBE2" signature for VBE 2.0+ info
        mov di, vbe_info
        mov cx, 256
        xor ax, ax
        rep stosw
        mov dword [vbe_info], 0x32454256        ; 'VBE2'

        mov ax, 0x4F00
        mov di, vbe_info
        int 0x10
        cmp ax, 0x004F
        je .vbe_ok

        mov si, msg_no_vbe
        call print_str
        mov ax, 0x4C01
        int 0x21

.vbe_ok:
        cmp dword [vbe_info], 0x41534556        ; 'VESA'
        je .sig_ok
        mov si, msg_bad_sig
        call print_str
        mov ax, 0x4C01
        int 0x21

.sig_ok:
        ; Print VBEPROBE: ver=M.m vram=<KB>KB caps=0xXXXXXXXX
        mov si, msg_ver
        call print_str
        movzx ax, byte [vbe_info + 5]           ; major
        call print_dec16
        mov al, '.'
        call print_char
        movzx ax, byte [vbe_info + 4]           ; minor
        call print_dec16

        mov si, msg_vram
        call print_str
        movzx eax, word [vbe_info + 18]         ; TotalMemory in 64 KiB blocks
        shl eax, 6                              ; -> KiB
        call print_dec32
        mov si, msg_kb
        call print_str

        mov si, msg_caps
        call print_str
        mov eax, [vbe_info + 10]                ; Capabilities
        call print_hex32

        ; Query current mode (AX=4F03h)
        mov ax, 0x4F03
        int 0x10
        cmp ax, 0x004F
        jne .cur_skip
        mov si, msg_cur
        call print_str
        mov ax, bx
        call print_hex16
.cur_skip:
        call print_crlf

        ; Print OEM string at far pointer [vbe_info + 6] (off, seg)
        mov si, msg_oem
        call print_str
        mov ax, [vbe_info + 8]                  ; seg
        mov bx, [vbe_info + 6]                  ; off
        or ax, bx
        jz .oem_done
        push ds
        mov ds, [cs:vbe_info + 8]
        mov si, [cs:vbe_info + 6]
        mov cx, 64
.oem_loop:
        lodsb
        test al, al
        jz .oem_pop
        cmp al, 0x20
        jb .oem_next
        cmp al, 0x7E
        ja .oem_next
        call print_char
.oem_next:
        loop .oem_loop
.oem_pop:
        pop ds
.oem_done:
        call print_crlf

        ; Query VBE 2.0 Protected-Mode Interface (AX=4F0Ah BL=0)
        xor di, di
        mov es, di
        xor cx, cx
        mov ax, 0x4F0A
        xor bx, bx
        int 0x10
        mov [pm_ax], ax
        mov [pm_seg], es
        mov [pm_off], di
        mov [pm_len], cx
        push cs
        pop es

        mov si, msg_pmi
        call print_str
        cmp word [pm_ax], 0x004F
        jne .pmi_none
        mov si, msg_pmi_yes
        call print_str
        mov ax, [pm_seg]
        call print_hex16
        mov al, ':'
        call print_char
        mov ax, [pm_off]
        call print_hex16
        mov si, msg_pmi_len
        call print_str
        mov ax, [pm_len]
        call print_dec16
        call print_crlf
        jmp .walk_modes

.pmi_none:
        mov si, msg_pmi_no
        call print_str

.walk_modes:
        ; Copy VideoModePtr list into local mode_list (up to 128 modes) before
        ; calling INT 10h AX=4F01h, because some BIOSes reuse a scratch buffer.
        mov ax, [vbe_info + 16]                 ; VideoModePtr seg
        mov si, [vbe_info + 14]                 ; VideoModePtr off
        mov dx, ax
        or dx, si
        jz .modes_done

        push ds
        mov ds, ax
        mov di, mode_list
        xor cx, cx
.copy_modes:
        cmp cx, 128
        jae .copy_done
        lodsw
        cmp ax, 0xFFFF
        je .copy_done
        stosw
        inc cx
        jmp .copy_modes
.copy_done:
        pop ds
        mov [mode_count], cx

        mov si, msg_hdr
        call print_str

        xor bx, bx                              ; index
        mov word [gfx_count], 0
        mov word [lfb_count], 0

.mode_loop:
        cmp bx, [mode_count]
        jae .modes_done
        mov si, bx
        shl si, 1
        mov cx, [mode_list + si]
        mov [cur_mode], cx

        ; Zero 256-byte ModeInfoBlock
        push cx
        mov di, mode_info
        mov cx, 128
        xor ax, ax
        rep stosw
        pop cx

        mov ax, 0x4F01
        mov di, mode_info
        int 0x10
        cmp ax, 0x004F
        jne .next_mode

        ; Filter: ModeAttributes bit 0 = supported, bit 4 = graphics mode
        mov ax, [mode_info + 0]
        test ax, 0x0001
        jz .next_mode
        test ax, 0x0010
        jz .next_mode

        ; Filter out tiny/bogus resolutions (< 320x200) or 0 bpp
        cmp word [mode_info + 18], 320          ; XResolution
        jb .next_mode
        cmp word [mode_info + 20], 200          ; YResolution
        jb .next_mode
        cmp byte [mode_info + 25], 4            ; BitsPerPixel
        jb .next_mode

        inc word [gfx_count]
        test word [mode_info + 0], 0x0080       ; LFB supported (bit 7)
        jz .no_lfb_inc
        inc word [lfb_count]
.no_lfb_inc:

        ; Print: VBEPROBE: mode=0xXXXX WxHxBPP pitch=P flags=LFB/BANK lfb=0xXXXXXXXX
        mov si, msg_mode
        call print_str
        mov ax, [cur_mode]
        call print_hex16
        mov al, ' '
        call print_char
        mov ax, [mode_info + 18]                ; XResolution
        call print_dec16
        mov al, 'x'
        call print_char
        mov ax, [mode_info + 20]                ; YResolution
        call print_dec16
        mov al, 'x'
        call print_char
        movzx ax, byte [mode_info + 25]         ; BitsPerPixel
        call print_dec16

        mov si, msg_pitch
        call print_str
        mov ax, [mode_info + 16]                ; BytesPerScanLine
        call print_dec16

        mov si, msg_flags
        call print_str
        mov ax, [mode_info + 0]
        test ax, 0x0080
        jz .chk_bank
        mov si, msg_lfb
        call print_str
        jmp .print_phys
.chk_bank:
        mov si, msg_bank
        call print_str
.print_phys:
        mov si, msg_phys
        call print_str
        mov eax, [mode_info + 40]               ; PhysBasePtr
        call print_hex32
        call print_crlf

.next_mode:
        inc bx
        jmp .mode_loop

.modes_done:
        mov si, msg_sum_a
        call print_str
        mov ax, [gfx_count]
        call print_dec16
        mov si, msg_sum_b
        call print_str
        mov ax, [lfb_count]
        call print_dec16
        call print_crlf

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

print_hex8:
        push ax
        push cx
        mov cl, al
        shr al, 4
        call .nib
        mov al, cl
        and al, 0x0F
        call .nib
        pop cx
        pop ax
        ret
.nib:
        add al, '0'
        cmp al, '9'
        jbe .out
        add al, 7
.out:
        call print_char
        ret

print_hex16:
        push ax
        xchg al, ah
        call print_hex8
        xchg al, ah
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

print_dec16:
        push ax
        push bx
        push cx
        push dx
        xor cx, cx
        mov bx, 10
.d16:
        xor dx, dx
        div bx
        push dx
        inc cx
        test ax, ax
        jnz .d16
.o16:
        pop dx
        add dl, '0'
        mov al, dl
        call print_char
        loop .o16
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
        xor cx, cx
        mov ebx, 10
.d32:
        xor edx, edx
        div ebx
        push dx
        inc cx
        test eax, eax
        jnz .d32
.o32:
        pop dx
        add dl, '0'
        mov al, dl
        call print_char
        loop .o32
        pop edx
        pop cx
        pop ebx
        pop eax
        ret

pm_ax:          dw 0
pm_seg:         dw 0
pm_off:         dw 0
pm_len:         dw 0
mode_count:     dw 0
cur_mode:       dw 0
gfx_count:      dw 0
lfb_count:      dw 0

msg_banner:     db '=== Ferric DOS VBEPROBE (Stage G read-only VBE recon) ===', 13, 10, '$'
msg_no_vbe:     db 'VBEPROBE: ERROR VESA VBE BIOS not detected (INT 10h 4F00h failed)', 13, 10, '$'
msg_bad_sig:    db 'VBEPROBE: ERROR invalid VESA signature', 13, 10, '$'
msg_ver:        db 'VBEPROBE: ver=$'
msg_vram:       db ' vram=$'
msg_kb:         db 'KB$'
msg_caps:       db ' caps=0x$'
msg_cur:        db ' cur=0x$'
msg_oem:        db 'VBEPROBE: oem=$'
msg_pmi:        db 'VBEPROBE: pm_iface=$'
msg_pmi_yes:    db 'YES ptr=$'
msg_pmi_len:    db ' len=$'
msg_pmi_no:     db 'NO', 13, 10, '$'
msg_hdr:        db 'VBEPROBE: graphics modes:', 13, 10, '$'
msg_mode:       db '  mode=0x$'
msg_pitch:      db ' pitch=$'
msg_flags:      db ' $'
msg_lfb:        db 'LFB $'
msg_bank:       db 'BANK$'
msg_phys:       db ' phys=0x$'
msg_sum_a:      db 'VBEPROBE: summary gfx_modes=$'
msg_sum_b:      db ' lfb_modes=$'
msg_crlf:       db 13, 10, '$'

        align 16
vbe_info:       times 512 db 0
mode_info:      times 256 db 0
mode_list:      times 128 dw 0
