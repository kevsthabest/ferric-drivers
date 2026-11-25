; Ferric DOS Gate 4a — Visual Audio Mixer TUI (MIXER.COM)
;
; Pure 16-bit NASM DOS .COM utility communicating with resident FERRIC.JLM
; via V86 API AX=0x002A (gate1_sb_mixer_ctl).
; Controls:
;   Up / Down         Select channel (AC'97 Speaker, SB Master, Voice/SFX, FM/Music)
;   Left / Right (-/+) Adjust selected channel live
;   M                 Mute / unmute selected channel
;   R                 Reset all channels to defaults (7 / 12 / 12 / 12)
;   Q / ESC           Exit cleanly

org 0x0100
bits 16
cpu 386

GATE1_DEVICE_ID equ 0xFD01

start:
        ; Locate FERRIC.JLM V86 entry point (INT 2Fh AX=1684h BX=0xFD01)
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, GATE1_DEVICE_ID
        int 0x2F
        mov ax, es
        or ax, di
        jnz .have_jlm
        mov dx, msg_no_jlm
        mov ah, 0x09
        int 0x21
        mov ax, 0x4C01
        int 0x21

.have_jlm:
        mov [jlm_off], di
        mov [jlm_seg], es

        ; Fetch initial mixer state (cmd=0)
        xor bx, bx
        xor cx, cx
        call call_mixer

        ; Hide text cursor (INT 10h AH=01h, CH=0x20)
        mov ah, 0x01
        mov cx, 0x2000
        int 0x10

        ; Clear screen with deep-blue background (attr 0x17)
        call clear_screen

.ui_loop:
        call draw_ui

        ; Wait for keystroke (INT 16h AH=00h -> AH=scan, AL=ascii)
        xor ah, ah
        int 0x16

        cmp al, 27              ; ESC
        je .exit_ok
        cmp al, 'q'
        je .exit_ok
        cmp al, 'Q'
        je .exit_ok

        cmp ah, 0x48            ; Up arrow
        je .key_up
        cmp ah, 0x50            ; Down arrow
        je .key_down
        cmp ah, 0x4B            ; Left arrow
        je .key_dec
        cmp al, '-'
        je .key_dec
        cmp ah, 0x4D            ; Right arrow
        je .key_inc
        cmp al, '+'
        je .key_inc
        cmp al, '='
        je .key_inc
        cmp al, 'm'
        je .key_mute
        cmp al, 'M'
        je .key_mute
        cmp al, 'r'
        je .key_reset
        cmp al, 'R'
        je .key_reset
        jmp .ui_loop

.key_up:
        cmp byte [sel_ch], 0
        je .ui_loop
        dec byte [sel_ch]
        jmp .ui_loop

.key_down:
        cmp byte [sel_ch], 3
        jae .ui_loop
        inc byte [sel_ch]
        jmp .ui_loop

.key_dec:
        movzx si, byte [sel_ch]
        mov al, [ch_vals + si]
        or al, al
        jz .ui_loop
        dec al
        jmp .apply_val

.key_inc:
        movzx si, byte [sel_ch]
        mov al, [ch_vals + si]
        cmp al, [ch_max + si]
        jae .ui_loop
        inc al
        jmp .apply_val

.key_mute:
        movzx si, byte [sel_ch]
        mov al, [ch_vals + si]
        or al, al
        jz .unmute
        mov [ch_saved + si], al
        xor al, al
        jmp .apply_val
.unmute:
        mov al, [ch_saved + si]
        or al, al
        jnz .apply_val
        mov al, [ch_def + si]
        jmp .apply_val

.key_reset:
        mov bx, 5               ; cmd=5: reset defaults
        xor cx, cx
        call call_mixer
        jmp .ui_loop

.apply_val:
        ; si = channel 0..3, al = new value
        mov bx, si
        inc bx                  ; cmd = 1..4
        movzx cx, al
        call call_mixer
        jmp .ui_loop

.exit_ok:
        ; Restore normal DOS screen (white on black) and cursor
        mov ax, 0x0600
        mov bh, 0x07
        xor cx, cx
        mov dx, 0x184F
        int 0x10
        mov ah, 0x02
        xor bh, bh
        xor dx, dx
        int 0x10
        mov ah, 0x01
        mov cx, 0x0607
        int 0x10
        mov ax, 0x4C00
        int 0x21

; Call FERRIC.JLM AX=0x002A (BX=cmd, CX=val) and unpack EAX into ch_vals[0..3].
call_mixer:
        mov ax, 0x002A
        call far [jlm_off]
        mov [ch_vals + 0], al
        mov [ch_vals + 1], ah
        shr eax, 16
        mov [ch_vals + 2], al
        mov [ch_vals + 3], ah
        ret

clear_screen:
        mov ax, 0x0600
        mov bh, 0x17
        xor cx, cx
        mov dx, 0x184F
        int 0x10
        ret

; Draw the full TUI frame + 4 channel sliders directly to text video RAM (0xB800).
draw_ui:
        push es
        mov ax, 0xB800
        mov es, ax

        ; Top border (row 3, col 10..69)
        mov dh, 3
        mov dl, 10
        mov bh, 0x1F
        mov si, str_top
        call put_str_xy

        ; Title row (row 4)
        mov dh, 4
        mov dl, 10
        mov bh, 0x30            ; black on cyan title bar
        mov si, str_title
        call put_str_xy

        ; Separator (row 5)
        mov dh, 5
        mov dl, 10
        mov bh, 0x1F
        mov si, str_sep
        call put_str_xy

        ; Blank padding rows & 4 channel rows (rows 7, 10, 13, 16)
        mov dh, 6
        call draw_empty_row
        mov dh, 8
        call draw_empty_row
        mov dh, 9
        call draw_empty_row
        mov dh, 11
        call draw_empty_row
        mov dh, 12
        call draw_empty_row
        mov dh, 14
        call draw_empty_row
        mov dh, 15
        call draw_empty_row
        mov dh, 17
        call draw_empty_row

        xor cl, cl              ; channel index 0..3
.ch_loop:
        push cx
        call draw_channel_row
        pop cx
        inc cl
        cmp cl, 4
        jb .ch_loop

        ; Footer separator (row 18) & help bar (row 19) & bottom border (row 20)
        mov dh, 18
        mov dl, 10
        mov bh, 0x1F
        mov si, str_sep
        call put_str_xy

        mov dh, 19
        mov dl, 10
        mov bh, 0x1E            ; yellow on blue help text
        mov si, str_help
        call put_str_xy

        mov dh, 20
        mov dl, 10
        mov bh, 0x1F
        mov si, str_bot
        call put_str_xy

        pop es
        ret

; Draw empty interior box row at DH
draw_empty_row:
        mov dl, 10
        mov bh, 0x17
        mov si, str_empty
        call put_str_xy
        ret

; Draw one channel slider row for CL = 0..3.
draw_channel_row:
        movzx si, cl
        mov dh, [ch_rows + si]
        ; Pick highlight attribute if selected
        mov bh, 0x17            ; unselected: light gray on blue
        cmp cl, [sel_ch]
        jne .not_sel
        mov bh, 0x70            ; selected: black on light gray
.not_sel:
        ; Fill interior of row (cols 11..68) with row attribute
        mov dl, 10
        mov al, '|'
        push bx
        mov bh, 0x1F
        call put_char_xy
        pop bx
        mov dl, 69
        mov al, '|'
        push bx
        mov bh, 0x1F
        call put_char_xy
        pop bx

        mov dl, 11
        mov cx, 58
.fill_sp:
        mov al, ' '
        call put_char_xy
        inc dl
        loop .fill_sp

        ; Save channel index in DI (0..3) before using SI for string pointers
        mov di, si

        ; Selection pointer '>' at col 12
        mov dl, 12
        mov ax, di
        cmp al, [sel_ch]
        mov al, ' '
        jne .no_ptr
        mov al, '>'
.no_ptr:
        call put_char_xy

        ; Channel label at col 14 (BH already holds row attribute)
        mov si, di
        shl si, 1
        mov si, [ch_names + si]
        mov dl, 14
        call put_str_xy

        ; Draw slider bracket '[' at col 32, 28-cell bar (cols 33..60), ']' at col 61
        mov dl, 32
        mov al, '['
        call put_char_xy

        ; Filled cells = (val * 28) / max (preserve DX: DH=row!)
        push dx
        movzx ax, byte [ch_vals + di]
        mov cx, 28
        mul cx
        movzx cx, byte [ch_max + di]
        div cx                  ; AX = filled count (0..28)
        mov bp, ax              ; BP = filled count
        pop dx

        mov dl, 33
        xor cx, cx              ; cell 0..27
.bar_lp:
        mov al, '#'
        cmp cx, bp
        jb .bar_draw
        mov al, '.'
.bar_draw:
        call put_char_xy
        inc dl
        inc cx
        cmp cx, 28
        jb .bar_lp

        mov dl, 61
        mov al, ']'
        call put_char_xy

        ; Numeric value "NN/MM" at cols 63..67 (or "MUTE" at cols 64..67 when 0)
        mov al, [ch_vals + di]
        or al, al
        jnz .show_num
        mov dl, 64
        mov si, str_mute
        call put_str_xy
        ret

.show_num:
        mov dl, 63
        call put_dec2_xy
        mov al, '/'
        call put_char_xy
        inc dl
        mov al, [ch_max + di]
        cmp al, 10
        jb .one_dig
        call put_dec2_xy
        ret
.one_dig:
        add al, '0'
        call put_char_xy
        ret

; Print 2-digit decimal AL (0..99) at (DH, DL), advancing DL by 2.
put_dec2_xy:
        xor ah, ah
        mov bl, 10
        div bl                  ; AL = tens, AH = ones
        push ax
        or al, al
        jnz .tens_dig
        mov al, ' '
        jmp .tens_out
.tens_dig:
        add al, '0'
.tens_out:
        call put_char_xy
        inc dl
        pop ax
        mov al, ah
        add al, '0'
        call put_char_xy
        inc dl
        ret

; Write NUL-terminated string DS:SI at row DH, col DL with attribute BH.
put_str_xy:
        push ax
        push dx
.ps_lp:
        lodsb
        or al, al
        jz .ps_done
        call put_char_xy
        inc dl
        jmp .ps_lp
.ps_done:
        pop dx
        pop ax
        ret

; Write character AL with attribute BH at row DH, col DL into ES=0xB800.
put_char_xy:
        push ax
        push bx
        push dx
        push di
        push ax
        movzx ax, dh
        imul ax, ax, 80
        movzx di, dl
        add di, ax
        shl di, 1
        pop ax
        mov ah, bh
        stosw
        pop di
        pop dx
        pop bx
        pop ax
        ret

; ---------------------------------------------------------------------
; Data
; ---------------------------------------------------------------------
jlm_off:        dw 0
jlm_seg:        dw 0
sel_ch:         db 0
ch_vals:        db 7, 12, 12, 12
ch_saved:       db 7, 12, 12, 12
ch_max:         db 9, 15, 15, 15
ch_def:         db 7, 12, 12, 12
ch_rows:        db 7, 10, 13, 16
ch_names:       dw lbl_ac97, lbl_master, lbl_voice, lbl_fm

lbl_ac97:       db "AC'97 Speaker  ", 0
lbl_master:     db "SB Master      ", 0
lbl_voice:      db "SB Voice / SFX ", 0
lbl_fm:         db "SB FM / Music  ", 0
str_mute:       db "MUTE", 0

str_top:        db "+----------------------------------------------------------+", 0
str_title:      db "|          Ferric DOS Audio Mixer (ALi M5451 + SB)         |", 0
str_sep:        db "+----------------------------------------------------------+", 0
str_empty:      db "|                                                          |", 0
str_help:       db "| Up/Dn:Select  Lt/Rt:Vol  M:Mute  R:Reset  Q/ESC:Quit     |", 0
str_bot:        db "+----------------------------------------------------------+", 0

msg_no_jlm:     db "MIXER: FERRIC.JLM not loaded. Run JLOAD FERRIC.JLM first.", 13, 10, "$"
