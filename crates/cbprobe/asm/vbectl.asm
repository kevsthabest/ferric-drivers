; =============================================================================
; VBECTL.COM - Stage G Step 2: VBE 2.0 LFB Mode Switcher & DPMI Framebuffer Test
; =============================================================================
; Usage:
;   VBECTL [800|1024|640] [/Q]   Switch to VBE LFB mode, map PhysBasePtr via
;                                DPMI INT 31h AX=0800h (HDPMI32i), draw color
;                                bars + white border, wait for key (unless /Q),
;                                and restore 80x25 text mode (AX=0003h).
;   VBECTL TEXT                  Force restore to 80x25 text mode (AX=0003h).
;
; Strictly 386 baseline, read-only storage, safe text-mode restore.
; =============================================================================

cpu 386
bits 16
org 0x0100

start:
        push cs
        pop ds
        push cs
        pop es
        ; Set stack inside our own COM image BEFORE shrinking via AH=4Ah!
        mov sp, stack_top

        ; Parse command tail: optional 640 / 800 / 1024 / TEXT, /Q (no wait), /B (banked), /M (map-only)
        mov word [want_w], 800
        mov word [want_h], 600
        mov byte [quiet_mode], 0
        mov byte [bank_mode], 0
        mov byte [map_only], 0
        mov byte [accel_2d_mode], 0

        movzx cx, byte [0x0080]
        mov si, 0x0081
.arg_loop:
        test cx, cx
        jz .args_done
        lodsb
        dec cx
        cmp al, ' '
        je .arg_loop
        cmp al, 9
        je .arg_loop
        cmp al, 13
        je .args_done
        cmp al, '/'
        je .chk_flag
        cmp al, '-'
        je .chk_flag
        cmp al, '1'
        je .set_1024
        cmp al, '8'
        je .set_800
        cmp al, '6'
        je .set_640
        or al, 0x20
        cmp al, 't'
        je .chk_text
        jmp .skip_tok

.chk_flag:
        test cx, cx
        jz .args_done
        lodsb
        dec cx
        or al, 0x20
        cmp al, 'q'
        je .set_quiet
        cmp al, 'b'
        je .set_bank
        cmp al, 'm'
        je .set_map_only
        cmp al, '2'
        je .set_2d
        jmp .skip_tok
.set_quiet:
        mov byte [quiet_mode], 1
        jmp .arg_loop
.set_bank:
        mov byte [bank_mode], 1
        jmp .skip_tok
.set_map_only:
        mov byte [map_only], 1
        jmp .skip_tok
.set_2d:
        mov byte [accel_2d_mode], 1
        jmp .skip_tok

.set_1024:
        mov word [want_w], 1024
        mov word [want_h], 768
        jmp .skip_tok
.set_800:
        mov word [want_w], 800
        mov word [want_h], 600
        jmp .skip_tok
.set_640:
        mov word [want_w], 640
        mov word [want_h], 480
        jmp .skip_tok
.chk_text:
        ; Check if "text" (next char 'e' or 'E')
        test cx, cx
        jz .args_done
        mov al, [si]
        or al, 0x20
        cmp al, 'e'
        jne .skip_tok
        mov ax, 0x0003
        int 0x10
        mov si, msg_text_ok
        call print_str_rm
        mov ax, 0x4C00
        int 0x21

.skip_tok:
        test cx, cx
        jz .args_done
        lodsb
        dec cx
        cmp al, ' '
        je .arg_loop
        cmp al, 9
        je .arg_loop
        cmp al, 13
        je .args_done
        jmp .skip_tok

.args_done:
        ; 1. Query VBE 2.0 controller info (INT 10h AX=4F00h)
        mov di, vbe_info
        mov cx, 256
        xor ax, ax
        rep stosw
        mov dword [vbe_info], 0x32454256        ; 'VBE2'
        mov ax, 0x4F00
        mov di, vbe_info
        int 0x10
        cmp ax, 0x004F
        jne .no_vbe
        cmp dword [vbe_info], 0x41534556        ; 'VESA'
        jne .no_vbe

        ; Copy VideoModePtr list locally (up to 128 modes)
        mov ax, [vbe_info + 16]
        mov si, [vbe_info + 14]
        mov dx, ax
        or dx, si
        jz .no_mode
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

        ; Search for requested WxH with LFB (prefer 32bpp, fallback 16bpp)
        mov byte [want_bpp], 32
        call find_lfb_mode
        cmp word [sel_mode], 0
        jne .have_mode
        mov byte [want_bpp], 16
        call find_lfb_mode
        cmp word [sel_mode], 0
        jne .have_mode
        jmp .no_mode

.no_vbe:
        mov si, msg_no_vbe
        call print_str_rm
        mov ax, 0x4C01
        int 0x21

.no_mode:
        mov si, msg_no_mode
        call print_str_rm
        mov ax, 0x4C02
        int 0x21

.have_mode:
        ; Compute framebuffer size = pitch * height (bytes)
        movzx eax, word [sel_pitch]
        movzx edx, word [sel_h]
        mul edx
        mov [fb_size], eax

        ; If /2D, run VBE.JLM (0xFD04) Trident BladeXP 2D hardware accel test
        cmp byte [accel_2d_mode], 0
        jne do_vbe_jlm_2d

        ; If /B (banked pure real-mode test), skip DPMI completely and test
        ; VBE AX=4F02h (without LFB bit) + AX=4F05h window 0 at 0xA000:0000.
        cmp byte [bank_mode], 0
        jne .do_banked_test

        ; 2. Shrink our own COM memory block so DPMI private data alloc succeeds
        mov si, msg_step0
        call print_str_rm
        push cs
        pop es
        mov bx, prog_end
        add bx, 15
        shr bx, 4
        mov ah, 0x4A
        int 0x21

        ; 3. Detect DPMI host (INT 2Fh AX=1687h)
        mov ax, 0x1687
        int 0x2F
        test ax, ax
        jnz .no_dpmi
        test bl, 1                              ; 32-bit DPMI supported?
        jz .no_dpmi
        mov [dpmi_off], di
        mov [dpmi_seg], es
        mov [rm_cs], cs

        ; Allocate DPMI private data paragraphs if SI != 0
        test si, si
        jz .enter_dpmi
        mov bx, si
        mov ah, 0x48
        int 0x21
        jc .no_dpmi
        mov es, ax

.enter_dpmi:
        mov ax, 0x0001                          ; 32-bit protected mode client
        call far [dpmi_off]
        jc .no_dpmi

        ; =====================================================================
        ; Now in 16-bit code segment (CS/DS/ES/SS are valid DPMI selectors
        ; mapped to our COM segment; 32-bit operand overrides work cleanly).
        ; =====================================================================

        ; 4. Map physical LFB via DPMI INT 31h AX=0800h
        ;    BX:CX = phys_base, SI:DI = size_bytes -> returns BX:CX = lin_base
        mov dx, msg_step1
        call print_str_pm
        mov eax, [sel_phys]
        mov cx, ax
        shr eax, 16
        mov bx, ax
        mov eax, [fb_size]
        mov di, ax
        shr eax, 16
        mov si, ax
        mov ax, 0x0800
        int 0x31
        jc .pm_map_fail
        mov [lfb_lin + 0], cx
        mov [lfb_lin + 2], bx

        ; 5. Allocate 1 LDT descriptor (INT 31h AX=0000h) for the LFB
        mov dx, msg_step2
        call print_str_pm
        mov ax, 0x0000
        mov cx, 1
        int 0x31
        jc .pm_sel_fail
        mov [lfb_sel], ax

        ; Set selector base = lfb_lin (INT 31h AX=0007h, BX=sel, CX:DX=base)
        mov bx, [lfb_sel]
        mov dx, [lfb_lin + 0]
        mov cx, [lfb_lin + 2]
        mov ax, 0x0007
        int 0x31
        jc .pm_sel_free_fail

        ; Set 32-bit RW data access rights (INT 31h AX=0009h, CX=0xC0F3: G=1 D=1 P=1 DPL=3 RW)
        mov bx, [lfb_sel]
        mov cx, 0xC0F3
        mov ax, 0x0009
        int 0x31
        jc .pm_sel_free_fail

        ; Set selector limit = fb_size - 1 (page-granular if >= 1 MiB)
        mov eax, [fb_size]
        dec eax
        or eax, 0x00000FFF                      ; round up to 4 KiB boundary
        mov dx, ax
        shr eax, 16
        mov cx, ax
        mov bx, [lfb_sel]
        mov ax, 0x0008
        int 0x31
        jc .pm_sel_free_fail

        ; 6. If /M (map-only), read 1 dword from LFB and skip mode switch!
        cmp byte [map_only], 0
        je .do_mode_switch
        push es
        mov es, [lfb_sel]
        mov eax, [es:0]
        mov [readback_px], eax
        pop es
        jmp .cleanup_and_report

.do_mode_switch:
        ; Switch to VBE LFB mode via DPMI RM INT 10h (AX=4F02h, BX=mode|4000h)
        mov dx, msg_step3
        call print_str_pm
        call clear_rmregs
        mov word [rm_eax], 0x4F02
        mov ax, [sel_mode]
        or ax, 0x4000                           ; bit 14 = Linear Framebuffer
        mov [rm_ebx], ax
        mov bx, 0x0010
        call dpmi_rm_int
        jc .pm_mode_fail
        cmp word [rm_eax], 0x004F
        jne .pm_mode_fail

        ; 7. Draw 8 vertical color bars + 4-pixel white border directly to LFB!
        push es
        mov es, [lfb_sel]
        xor edi, edi
        movzx edx, word [sel_h]                 ; y = 0..H-1
        xor esi, esi                            ; row counter y
.row_loop:
        cmp si, [sel_h]
        jae .draw_done
        movzx ecx, word [sel_w]                 ; x = 0..W-1
        xor ebx, ebx                            ; col counter x
        push edi                                ; save row start offset
.col_loop:
        cmp bx, [sel_w]
        jae .row_next

        ; White 4px border
        cmp si, 4
        jb .pix_white
        mov ax, [sel_h]
        sub ax, 4
        cmp si, ax
        jae .pix_white
        cmp bx, 4
        jb .pix_white
        mov ax, [sel_w]
        sub ax, 4
        cmp bx, ax
        jae .pix_white

        ; Pick bar index = (x * 8) / width -> 0..7
        push dx
        mov ax, bx
        shl ax, 3
        xor dx, dx
        div word [sel_w]                        ; AX = bar 0..7
        pop dx
        mov bp, ax
        cmp byte [sel_bpp], 32
        je .bar_32
        shl bp, 1
        mov ax, [bar_colors_16 + bp]
        jmp .store_16
.bar_32:
        shl bp, 2
        mov eax, [bar_colors_32 + bp]
        jmp .store_32

.pix_white:
        cmp byte [sel_bpp], 32
        je .white_32
        mov ax, 0xFFFF
.store_16:
        mov [es:edi], ax
        add edi, 2
        inc bx
        jmp .col_loop
.white_32:
        mov eax, 0x00FFFFFF
.store_32:
        mov [es:edi], eax
        add edi, 4
        inc bx
        jmp .col_loop

.row_next:
        pop edi
        movzx eax, word [sel_pitch]
        add edi, eax
        inc si
        jmp .row_loop

.draw_done:
        ; Read back corner pixel (0,0) to verify live VRAM write/readback
        mov eax, [es:0]
        mov [readback_px], eax
        pop es

        ; 8. Wait for keypress unless /Q
        cmp byte [quiet_mode], 0
        jne .restore_text
        call clear_rmregs
        mov word [rm_eax], 0x0000
        mov bx, 0x0016
        call dpmi_rm_int

.restore_text:
        ; 9. Always restore 80x25 text mode (INT 10h AX=0003h)
        call clear_rmregs
        mov word [rm_eax], 0x0003
        mov bx, 0x0010
        call dpmi_rm_int

.cleanup_and_report:
        ; 10. Clean up LDT selector and physical mapping
        mov bx, [lfb_sel]
        mov ax, 0x0001
        int 0x31
        mov cx, [lfb_lin + 0]
        mov bx, [lfb_lin + 2]
        mov ax, 0x0801
        int 0x31

        ; 11. Format and print PASS report via DPMI RM INT 21h AH=09h
        mov di, out_buf
        mov si, msg_pass_a
        call append_str
        mov ax, [sel_mode]
        call append_hex16
        mov al, ' '
        call append_ch
        mov ax, [sel_w]
        call append_dec16
        mov al, 'x'
        call append_ch
        mov ax, [sel_h]
        call append_dec16
        mov al, 'x'
        call append_ch
        movzx ax, byte [sel_bpp]
        call append_dec16
        mov si, msg_pass_phys
        call append_str
        mov eax, [sel_phys]
        call append_hex32
        mov si, msg_pass_lin
        call append_str
        mov eax, [lfb_lin]
        call append_hex32
        mov si, msg_pass_px
        call append_str
        mov eax, [readback_px]
        call append_hex32
        mov si, msg_crlf
        call append_str
        mov byte [di], '$'

        mov dx, out_buf
        call print_str_pm
        mov ax, 0x4C00
        int 0x21

.no_dpmi:
        mov si, msg_no_dpmi
        call print_str_rm
        mov ax, 0x4C03
        int 0x21

.pm_map_fail:
        mov dx, msg_map_fail
        call print_str_pm
        mov ax, 0x4C04
        int 0x21

.pm_sel_free_fail:
        mov bx, [lfb_sel]
        mov ax, 0x0001
        int 0x31
.pm_sel_fail:
        mov cx, [lfb_lin + 0]
        mov bx, [lfb_lin + 2]
        mov ax, 0x0801
        int 0x31
        mov dx, msg_sel_fail
        call print_str_pm
        mov ax, 0x4C05
        int 0x21

.pm_mode_fail:
        mov bx, [lfb_sel]
        mov ax, 0x0001
        int 0x31
        mov cx, [lfb_lin + 0]
        mov bx, [lfb_lin + 2]
        mov ax, 0x0801
        int 0x31
        mov dx, msg_mode_fail
        call print_str_pm
        mov ax, 0x4C06
        int 0x21

; --- Pure real-mode banked VBE test (/B): no DPMI, writes 64 KiB bank 0 ---
.do_banked_test:
        mov ax, 0x4F02
        mov bx, [sel_mode]                      ; no bit 14 -> banked window
        int 0x10
        cmp ax, 0x004F
        jne .bank_fail
        ; Select bank 0
        mov ax, 0x4F05
        xor bx, bx
        xor dx, dx
        int 0x10
        ; Fill first 64 KiB (top band of screen) with bright cyan/white stripes
        push es
        mov ax, 0xA000
        mov es, ax
        xor di, di
        mov cx, 0x8000
        mov ax, 0x07FF
        rep stosw
        pop es
        cmp byte [quiet_mode], 0
        jne .bank_restore
        xor ax, ax
        int 0x16
.bank_restore:
        mov ax, 0x0003
        int 0x10
        mov si, msg_bank_ok
        call print_str_rm
        mov ax, 0x4C00
        int 0x21
.bank_fail:
        mov si, msg_mode_fail
        call print_str_rm
        mov ax, 0x4C06
        int 0x21

; --- Real-mode helper: find LFB mode matching want_w x want_h x want_bpp ---
find_lfb_mode:
        mov word [sel_mode], 0
        xor bx, bx
.fl_loop:
        cmp bx, [mode_count]
        jae .fl_done
        mov si, bx
        shl si, 1
        mov cx, [mode_list + si]
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
        jne .fl_next
        mov ax, [mode_info + 0]
        and ax, 0x0091                          ; supported(0) + gfx(4) + LFB(7)
        cmp ax, 0x0091
        jne .fl_next
        mov ax, [want_w]
        cmp [mode_info + 18], ax
        jne .fl_next
        mov ax, [want_h]
        cmp [mode_info + 20], ax
        jne .fl_next
        mov al, [want_bpp]
        cmp [mode_info + 25], al
        jne .fl_next
        cmp dword [mode_info + 40], 0
        je .fl_next
        mov [sel_mode], cx
        mov ax, [mode_info + 18]
        mov [sel_w], ax
        mov ax, [mode_info + 20]
        mov [sel_h], ax
        mov al, [mode_info + 25]
        mov [sel_bpp], al
        mov ax, [mode_info + 16]
        mov [sel_pitch], ax
        mov eax, [mode_info + 40]
        mov [sel_phys], eax
        ret
.fl_next:
        inc bx
        jmp .fl_loop
.fl_done:
        ret

; --- DPMI real-mode interrupt call helper (BX = int number) ---
clear_rmregs:
        push es
        push edi
        push ecx
        push ax
        push ds
        pop es
        xor edi, edi
        mov di, rm_regs
        mov cx, 25
        xor ax, ax
        rep stosw
        pop ax
        pop ecx
        pop edi
        pop es
        ret

dpmi_rm_int:
        push es
        push edi
        push ds
        pop es
        ; Supply our own 2 KiB real-mode stack so VBE BIOS INT 10h 4F02h
        ; never overflows HDPMI32i's tiny 512-byte default RMS stack!
        mov ax, [rm_cs]
        mov [rm_ss], ax
        mov word [rm_sp], rm_stack_top
        xor edi, edi
        mov di, rm_regs
        xor cx, cx
        mov ax, 0x0300
        int 0x31
        pop edi
        pop es
        ret

; Print '$'-terminated string at DX from PM via DPMI RM INT 21h AH=09h
print_str_pm:
        call clear_rmregs
        mov word [rm_eax], 0x0900
        mov [rm_edx], dx
        mov ax, [rm_cs]
        mov [rm_ds], ax
        mov bx, 0x0021
        call dpmi_rm_int
        ret

print_str_rm:
        push ax
        push dx
        mov dx, si
        mov ah, 0x09
        int 0x21
        pop dx
        pop ax
        ret

; --- String formatting helpers writing to [DI] ---
append_ch:
        mov [di], al
        inc di
        ret

append_str:
        push ax
.as_loop:
        lodsb
        cmp al, '$'
        je .as_done
        mov [di], al
        inc di
        jmp .as_loop
.as_done:
        pop ax
        ret

append_hex8:
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
        jbe .o
        add al, 7
.o:
        call append_ch
        ret

append_hex16:
        push ax
        xchg al, ah
        call append_hex8
        xchg al, ah
        call append_hex8
        pop ax
        ret

append_hex32:
        push eax
        shr eax, 16
        call append_hex16
        pop eax
        call append_hex16
        ret

append_dec16:
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
        call append_ch
        loop .o16
        pop dx
        pop cx
        pop bx
        pop ax
        ret

; =============================================================================
; /2D: VBE.JLM (DDB 0xFD04) Hardware 2D FillRect + BitBlt verification
; =============================================================================
do_vbe_jlm_2d:
        ; Discover VBE.JLM via INT 2Fh AX=1684h BX=0xFD04
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD04
        int 0x2F
        mov [vbejlm_off], di
        mov ax, es
        mov [vbejlm_seg], ax
        push cs
        pop es
        or ax, di
        jnz .have_vbejlm
        mov si, msg_no_vbejlm
        call print_str_rm
        mov ax, 0x4C08
        int 0x21

.have_vbejlm:
        ; Switch into VBE LFB mode first (while Trident MMIO is still off)
        mov ax, 0x4F02
        mov bx, [sel_mode]
        or bx, 0x4000
        int 0x10
        cmp ax, 0x004F
        je .v2d_mode_ok
        mov si, msg_mode_fail
        call print_str_rm
        mov ax, 0x4C06
        int 0x21

.v2d_mode_ok:
        ; Populate VbeModeDesc (24 bytes) and call VBE_OP_SET_LFB (AX=0x0002)
        mov ax, [sel_w]
        mov [vbe_mode_desc + 0], ax
        mov ax, [sel_h]
        mov [vbe_mode_desc + 2], ax
        movzx eax, word [sel_pitch]
        mov [vbe_mode_desc + 4], eax
        mov al, [sel_bpp]
        mov [vbe_mode_desc + 8], al
        cmp al, 16
        je .v2d_sh16
        mov byte [vbe_mode_desc + 9], 16
        mov byte [vbe_mode_desc + 10], 8
        mov byte [vbe_mode_desc + 11], 0
        jmp .v2d_sh_done
.v2d_sh16:
        mov byte [vbe_mode_desc + 9], 11
        mov byte [vbe_mode_desc + 10], 5
        mov byte [vbe_mode_desc + 11], 0
.v2d_sh_done:
        mov eax, [sel_phys]
        mov [vbe_mode_desc + 12], eax
        mov eax, [fb_size]
        mov [vbe_mode_desc + 16], eax
        mov ax, [sel_mode]
        or ax, 0x4000
        mov [vbe_mode_desc + 20], ax
        mov word [vbe_mode_desc + 22], 0

        push cs
        pop es
        mov di, vbe_mode_desc
        mov ax, 0x0002
        call far [vbejlm_off]
        push cs
        pop es
        mov [rc_setlfb], ax
        test ax, ax
        jnz .v2d_restore

        ; Query VBE_OP_GET_CAPS (AX=0x0001) to confirm backend (TRIDENT_8820 vs SW)
        mov di, vbe_caps_buf
        mov ax, 0x0001
        call far [vbejlm_off]
        push cs
        pop es

        ; 1) Fill full screen dark navy
        mov word [vbe_fill_req + 0], 0
        mov word [vbe_fill_req + 2], 0
        mov ax, [sel_w]
        mov [vbe_fill_req + 4], ax
        mov ax, [sel_h]
        mov [vbe_fill_req + 6], ax
        mov dword [vbe_fill_req + 8], 0x00102040
        cmp byte [sel_bpp], 16
        jne .v2d_c0
        mov dword [vbe_fill_req + 8], 0x00001108
.v2d_c0:
        mov dword [vbe_fill_req + 12], 0xF0
        mov di, vbe_fill_req
        mov ax, 0x0003
        call far [vbejlm_off]
        push cs
        pop es
        mov [rc_fill], ax

        ; 2) Draw Red, Green, Blue boxes across top row (y=40, h=120)
        mov word [vbe_fill_req + 0], 40
        mov word [vbe_fill_req + 2], 40
        mov word [vbe_fill_req + 4], 180
        mov word [vbe_fill_req + 6], 120
        mov dword [vbe_fill_req + 8], 0x00FF2020
        cmp byte [sel_bpp], 16
        jne .v2d_c1
        mov dword [vbe_fill_req + 8], 0x0000F800
.v2d_c1:
        mov di, vbe_fill_req
        mov ax, 0x0003
        call far [vbejlm_off]
        push cs
        pop es

        mov word [vbe_fill_req + 0], 240
        mov dword [vbe_fill_req + 8], 0x0020FF20
        cmp byte [sel_bpp], 16
        jne .v2d_c2
        mov dword [vbe_fill_req + 8], 0x000007E0
.v2d_c2:
        mov di, vbe_fill_req
        mov ax, 0x0003
        call far [vbejlm_off]
        push cs
        pop es

        mov word [vbe_fill_req + 0], 440
        mov dword [vbe_fill_req + 8], 0x002080FF
        cmp byte [sel_bpp], 16
        jne .v2d_c3
        mov dword [vbe_fill_req + 8], 0x0000001F
.v2d_c3:
        mov di, vbe_fill_req
        mov ax, 0x0003
        call far [vbejlm_off]
        push cs
        pop es

        ; 3) Hardware BitBlt top row (40,40,580,120) -> (40,200)
        mov word [vbe_blit_req + 0], 40
        mov word [vbe_blit_req + 2], 40
        mov word [vbe_blit_req + 4], 40
        mov word [vbe_blit_req + 6], 200
        mov word [vbe_blit_req + 8], 580
        mov word [vbe_blit_req + 10], 120
        mov dword [vbe_blit_req + 12], 0xCC
        mov di, vbe_blit_req
        mov ax, 0x0004
        call far [vbejlm_off]
        push cs
        pop es
        mov [rc_blt], ax

        ; 4) Sync 2D GE (AX=0x0005)
        mov ax, 0x0005
        call far [vbejlm_off]
        push cs
        pop es
        mov [rc_sync], ax

        ; Wait for key unless /Q
        cmp byte [quiet_mode], 0
        jne .v2d_restore
        xor ax, ax
        int 0x16

.v2d_restore:
        ; Mandatory on Trident 8820: release HW MMIO before INT 10h text restore!
        mov ax, 0x0006
        call far [vbejlm_off]
        push cs
        pop es
        mov [rc_rel], ax

        mov ax, 0x0003
        int 0x10

        ; Format and print single-line verdict
        mov di, out_buf
        mov si, msg_2d_hdr
        call append_str
        cmp byte [vbe_caps_buf + 13], 2
        je .v2d_trident_lbl
        mov si, msg_2d_sw
        jmp .v2d_emit_be
.v2d_trident_lbl:
        mov si, msg_2d_hw
.v2d_emit_be:
        call append_str
        mov si, msg_2d_pci
        call append_str
        mov ax, [vbe_caps_buf + 8]
        call append_hex16
        mov al, ':'
        call append_ch
        mov ax, [vbe_caps_buf + 10]
        call append_hex16
        mov si, msg_2d_mmio
        call append_str
        mov eax, [vbe_caps_buf + 16]
        call append_hex32
        mov si, msg_2d_rcs
        call append_str
        mov ax, [rc_setlfb]
        call append_dec16
        mov al, '/'
        call append_ch
        mov ax, [rc_fill]
        call append_dec16
        mov al, '/'
        call append_ch
        mov ax, [rc_blt]
        call append_dec16
        mov al, '/'
        call append_ch
        mov ax, [rc_sync]
        call append_dec16
        mov al, '/'
        call append_ch
        mov ax, [rc_rel]
        call append_dec16
        mov al, 13
        call append_ch
        mov al, 10
        call append_ch
        mov al, '$'
        call append_ch

        mov si, out_buf
        call print_str_rm
        mov ax, 0x4C00
        int 0x21

; --- Data ---
want_w:         dw 800
want_h:         dw 600
want_bpp:       db 32
quiet_mode:     db 0
bank_mode:      db 0
map_only:       db 0
accel_2d_mode:  db 0
vbejlm_off:     dw 0
vbejlm_seg:     dw 0
rc_setlfb:      dw 0
rc_fill:        dw 0
rc_blt:         dw 0
rc_sync:        dw 0
rc_rel:         dw 0
vbe_mode_desc:  times 24 db 0
vbe_caps_buf:   times 40 db 0
vbe_fill_req:   times 16 db 0
vbe_blit_req:   times 16 db 0

mode_count:     dw 0
sel_mode:       dw 0
sel_w:          dw 0
sel_h:          dw 0
sel_bpp:        db 0
sel_pitch:      dw 0
sel_phys:       dd 0
fb_size:        dd 0
lfb_lin:        dd 0
lfb_sel:        dw 0
readback_px:    dd 0

dpmi_off:       dw 0
dpmi_seg:       dw 0
rm_cs:          dw 0

; 8 vertical SMPTE-like bars: White, Yellow, Cyan, Green, Magenta, Red, Blue, DarkGray
bar_colors_32:
        dd 0x00FFFFFF, 0x00FFFF00, 0x0000FFFF, 0x0000FF00
        dd 0x00FF00FF, 0x00FF0000, 0x000000FF, 0x00202020
bar_colors_16:
        dw 0xFFFF, 0xFFE0, 0x07FF, 0x07E0
        dw 0xF81F, 0xF800, 0x001F, 0x2104

msg_no_vbe:     db 'VBECTL: ERROR VBE 2.0 BIOS not detected', 13, 10, '$'
msg_no_mode:    db 'VBECTL: ERROR requested LFB mode not found', 13, 10, '$'
msg_no_dpmi:    db 'VBECTL: ERROR 32-bit DPMI host (HDPMI32i) not loaded', 13, 10, '$'
msg_map_fail:   db 'VBECTL: ERROR DPMI 0800h LFB physical mapping failed', 13, 10, '$'
msg_sel_fail:   db 'VBECTL: ERROR DPMI LDT selector setup failed', 13, 10, '$'
msg_mode_fail:  db 'VBECTL: ERROR VBE AX=4F02h mode switch failed', 13, 10, '$'
msg_text_ok:    db 'VBECTL: restored 80x25 text mode (0x03)', 13, 10, '$'
msg_bank_ok:    db 'VBECTL: BANKED mode switch + 0xA000 write PASS', 13, 10, '$'
msg_step0:      db 'VBECTL: [0] VBE mode found, entering DPMI...', 13, 10, '$'
msg_step1:      db 'VBECTL: [1] DPMI entered, mapping LFB...', 13, 10, '$'
msg_step2:      db 'VBECTL: [2] LFB mapped, setting 32-bit LDT selector...', 13, 10, '$'
msg_step3:      db 'VBECTL: [3] Selector ready, switching VBE LFB mode...', 13, 10, '$'
msg_pass_a:     db 'VBECTL: PASS mode=0x$'
msg_pass_phys:  db ' phys=0x$'
msg_pass_lin:   db ' lin=0x$'
msg_pass_px:    db ' px0=0x$'
msg_no_vbejlm:  db 'VBECTL: VBE.JLM (0xFD04) not loaded (run JLOAD VBE.JLM first)', 13, 10, '$'
msg_2d_hdr:     db 'VBECTL 2D: backend=$'
msg_2d_hw:      db 'TRIDENT_8820$'
msg_2d_sw:      db 'SW_2D$'
msg_2d_pci:     db ' pci=$'
msg_2d_mmio:    db ' mmio=0x$'
msg_2d_rcs:     db ' rc(set/fill/blt/sync/rel)=$'
msg_crlf:       db 13, 10, '$'

; DPMI Real-Mode Call Structure (50 bytes)
        align 4
rm_regs:
rm_edi:         dd 0
rm_esi:         dd 0
rm_ebp:         dd 0
rm_res:         dd 0
rm_ebx:         dd 0
rm_edx:         dd 0
rm_ecx:         dd 0
rm_eax:         dd 0
rm_flags:       dw 0
rm_es:          dw 0
rm_ds:          dw 0
rm_fs:          dw 0
rm_gs:          dw 0
rm_ip:          dw 0
rm_cs_reg:      dw 0
rm_sp:          dw 0
rm_ss:          dw 0

out_buf:        times 128 db 0

        align 16
vbe_info:       times 512 db 0
mode_info:      times 256 db 0
mode_list:      times 128 dw 0
        align 16
stack_base:     times 2048 db 0
stack_top:
        align 16
rm_stack_base:  times 2048 db 0
rm_stack_top:
prog_end:
