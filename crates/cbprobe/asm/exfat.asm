; =============================================================================
; EXFAT.COM - exFAT work item: USB Mass Storage Directory & Copier for exFAT media
; (JLM verbs 0x001B-0x001D; the FAT sibling USBCP.COM uses 0x0012-0x0014)
; =============================================================================
; Usage:
;   EXFAT                        List root directory of USB flash drive
;   EXFAT <usb_path> [dos_dest]  Copy file from USB drive to DOS
;   EXFAT /E                     Flush DOS + USB cache and safely eject drive
;
; Requires USBPROBE to be run first (initializes OHCI + mounts USB drive).
; Strictly 386 baseline.
; =============================================================================

cpu 386
bits 16
org 0x0100

start:
        push cs
        pop ds
        push cs
        pop es

        ; Discover FERRIC.JLM (0xFD01, used for /E eject) and EXFAT.JLM (0xFD02,
        ; used for exFAT dir/open/read verbs 0x001B-0x001D).
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD01
        int 0x2F
        mov [core_off], di
        mov ax, es
        mov [core_seg], ax

        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD02
        int 0x2F
        mov [jlm_off], di
        mov ax, es
        mov [jlm_seg], ax

        push cs
        pop es

        ; Parse PSP command tail at 0x0080 -> arg1 (src_name), optional arg2 (dst_name)
        movzx cx, byte [0x0080]
        test cx, cx
        jz .do_dir
        mov si, 0x0081

.skip1:
        test cx, cx
        jz .do_dir
        lodsb
        dec cx
        cmp al, ' '
        je .skip1
        cmp al, 9
        je .skip1
        cmp al, 13
        je .do_dir

        ; Copy arg1 into src_name (NUL-terminated)
        mov di, src_name
.copy1:
        stosb
        test cx, cx
        jz .end_arg1
        lodsb
        dec cx
        cmp al, ' '
        je .end_arg1
        cmp al, 9
        je .end_arg1
        cmp al, 13
        je .end_arg1
        jmp .copy1
.end_arg1:
        xor al, al
        stosb

        ; Check for safe-eject flag: /E, /e, -E, -e
        mov al, [src_name]
        cmp al, '/'
        je .chk_e_flag
        cmp al, '-'
        jne .skip2
.chk_e_flag:
        mov al, [src_name + 1]
        or al, 0x20
        cmp al, 'e'
        jne .skip2
        cmp byte [src_name + 2], 0
        je .do_eject

        ; Skip whitespace before optional arg2
.skip2:
        test cx, cx
        jz .default_dst
        lodsb
        dec cx
        cmp al, ' '
        je .skip2
        cmp al, 9
        je .skip2
        cmp al, 13
        je .default_dst

        ; Copy arg2 into dst_name
        mov di, dst_name
.copy2:
        stosb
        test cx, cx
        jz .end_arg2
        lodsb
        dec cx
        cmp al, ' '
        je .end_arg2
        cmp al, 9
        je .end_arg2
        cmp al, 13
        je .end_arg2
        jmp .copy2
.end_arg2:
        xor al, al
        stosb
        jmp .do_copy

.default_dst:
        ; Default destination = same filename as src_name (after last '\' or '/')
        mov si, src_name
        mov bx, si
.find_base:
        lodsb
        test al, al
        jz .copy_base
        cmp al, '\'
        je .upd_base
        cmp al, '/'
        jne .find_base
.upd_base:
        mov bx, si
        jmp .find_base
.copy_base:
        mov si, bx
        mov di, dst_name
.cb_loop:
        lodsb
        stosb
        test al, al
        jnz .cb_loop
        jmp .do_copy

.do_eject:
        mov ax, [core_seg]
        or ax, [core_off]
        jnz .eject_have_core
        mov si, msg_no_core
        call print_str
        mov ax, 0x4C01
        int 0x21
.eject_have_core:
        ; Flush all dirty FreeDOS disk buffers before flushing & ejecting USB MSC
        mov ah, 0x0D
        int 0x21
        mov ax, 0x0019                  ; FERRIC.JLM gate1_usb_msc_eject
        call far [core_off]
        test ax, ax
        jnz .eject_ok
        mov si, msg_no_msc
        call print_str
        mov ax, 0x4C02
        int 0x21
.eject_ok:
        mov si, msg_ejected
        call print_str
        mov ax, 0x4C00
        int 0x21

.do_dir:
        mov ax, [jlm_seg]
        or ax, [jlm_off]
        jnz .dir_have_jlm
        mov si, msg_no_jlm
        call print_str
        mov ax, 0x4C01
        int 0x21
.dir_have_jlm:
        mov di, io_buf
        mov cx, 2048
        mov ax, 0x001B
        call far [jlm_off]
        test ax, ax
        jnz .print_dir
        mov si, msg_no_msc
        call print_str
        mov ax, 0x4C02
        int 0x21
.print_dir:
        mov cx, ax
        mov si, io_buf
.dir_ch:
        lodsb
        call print_char
        loop .dir_ch
        mov ax, 0x4C00
        int 0x21

.do_copy:
        mov ax, [jlm_seg]
        or ax, [jlm_off]
        jnz .copy_have_jlm
        mov si, msg_no_jlm
        call print_str
        mov ax, 0x4C01
        int 0x21
.copy_have_jlm:
        ; Open source file on read-only USB drive (AX=0x001C, ES:DI=src_name)
        mov di, src_name
        mov ax, 0x001C
        call far [jlm_off]
        test ax, ax
        jnz .src_ok
        mov si, msg_not_found
        call print_str
        mov ax, 0x4C03
        int 0x21

.src_ok:
        ; Create destination file via standard DOS INT 21h AH=3Ch
        mov dx, dst_name
        xor cx, cx
        mov ah, 0x3C
        int 0x21
        jnc .dst_ok
        mov si, msg_dst_err
        call print_str
        mov ax, 0x4C04
        int 0x21

.dst_ok:
        mov [dst_handle], ax
        mov dword [total_bytes], 0

.copy_loop:
        ; Read up to 512 bytes from USB file (AX=0x0014, ES:DI=io_buf)
        mov di, io_buf
        mov ax, 0x001D
        call far [jlm_off]
        cmp ax, 0xFFFF
        je .read_err
        test ax, ax
        jz .copy_done

        ; Write AX bytes to DOS destination handle (INT 21h AH=40h)
        mov cx, ax
        mov bx, [dst_handle]
        mov dx, io_buf
        mov ah, 0x40
        int 0x21
        jc .write_err
        movzx eax, ax
        add [total_bytes], eax
        jmp .copy_loop

.copy_done:
        mov bx, [dst_handle]
        mov ah, 0x3E
        int 0x21

        mov si, msg_copied
        call print_str
        mov eax, [total_bytes]
        call print_dec32
        mov si, msg_bytes
        call print_str
        mov ax, 0x4C00
        int 0x21

.read_err:
        mov bx, [dst_handle]
        mov ah, 0x3E
        int 0x21
        mov si, msg_read_err
        call print_str
        mov ax, 0x4C05
        int 0x21

.write_err:
        mov bx, [dst_handle]
        mov ah, 0x3E
        int 0x21
        mov si, msg_write_err
        call print_str
        mov ax, 0x4C06
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

print_dec32:
        push eax
        push ebx
        push cx
        push edx
        xor cx, cx
        mov ebx, 10
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

core_off:     dw 0
core_seg:     dw 0
jlm_off:      dw 0
jlm_seg:      dw 0
dst_handle:   dw 0
total_bytes:  dd 0

msg_no_core:  db 'EXFAT: ERROR FERRIC.JLM (0xFD01) not loaded', 13, 10, '$'
msg_no_jlm:   db 'EXFAT: EXFAT.JLM (0xFD02) not loaded (run FSLOAD first)', 13, 10, '$'
msg_no_msc:   db 'EXFAT: No USB storage drive mounted (run USBPROBE first)', 13, 10, '$'
msg_not_found: db 'EXFAT: File not found on USB drive', 13, 10, '$'
msg_dst_err:  db 'EXFAT: Could not create destination file', 13, 10, '$'
msg_read_err: db 'EXFAT: USB read error', 13, 10, '$'
msg_write_err: db 'EXFAT: DOS write error', 13, 10, '$'
msg_copied:   db 'EXFAT: Copied $'
msg_bytes:    db ' bytes', 13, 10, '$'
msg_ejected:  db 'EXFAT: USB drive flushed and safely ejected.', 13, 10, '$'

src_name:     times 128 db 0
dst_name:     times 128 db 0
io_buf:       equ $
