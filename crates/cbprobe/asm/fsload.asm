; =============================================================================
; FSLOAD.COM - Stage F Modular Filesystem Loader (detect-and-load)
; =============================================================================
; Usage:
;   FSLOAD       Query FERRIC.JLM (0xFD01) AX=0x0035 and JLOAD matching module
;   FSLOAD /U    Unload loadable filesystem modules (EXFAT.JLM / NTFS.JLM)
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

        ; Shrink our own DOS memory block to just our COM + 512B stack so
        ; INT 21h AH=4Bh (EXEC JLOAD.EXE) has plenty of conventional memory.
        mov sp, stack_top
        mov bx, stack_top
        add bx, 15
        shr bx, 4
        mov ah, 0x4A
        int 0x21

        ; Discover FERRIC.JLM core (0xFD01)
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD01
        int 0x2F
        mov [core_off], di
        mov ax, es
        mov [core_seg], ax
        or ax, di
        jnz .have_core

        mov si, msg_no_core
        call print_str
        mov ax, 0x4C01
        int 0x21

.have_core:
        push cs
        pop es

        ; Check command tail for /U or -U
        movzx cx, byte [0x0080]
        mov si, 0x0081
.arg_scan:
        test cx, cx
        jz .do_detect
        lodsb
        dec cx
        cmp al, ' '
        je .arg_scan
        cmp al, 9
        je .arg_scan
        cmp al, 13
        je .do_detect
        cmp al, '/'
        je .chk_u
        cmp al, '-'
        jne .do_detect
.chk_u:
        test cx, cx
        jz .do_detect
        lodsb
        or al, 0x20
        cmp al, 'u'
        je .do_unload

.do_detect:
        ; Query AX=0x0035 on FERRIC.JLM (0xFD01):
        ;   AL = primary fs_type (0..5)
        ;   AH = flags (bit 4 = BLK_FLAG_HAS_EXFAT, bit 5 = BLK_FLAG_HAS_NTFS)
        mov ax, 0x0035
        call far [core_off]
        mov [fs_primary], al
        mov [fs_flags], ah
        mov byte [found_any], 0

        ; 1. Core-resident FAT12/16/32
        cmp al, 1
        jb .chk_exfat
        cmp al, 3
        ja .chk_exfat
        mov byte [found_any], 1
        mov si, msg_fat
        call print_str

.chk_exfat:
        ; 2. exFAT (primary == 4 or secondary flag bit 4)
        cmp byte [fs_primary], 4
        je .do_exfat
        test byte [fs_flags], 0x10
        jz .chk_ntfs
.do_exfat:
        mov byte [found_any], 1
        ; Check if EXFAT.JLM (0xFD02) is already loaded
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD02
        int 0x2F
        mov ax, es
        or ax, di
        jz .exec_load_exfat
        push cs
        pop es
        mov si, msg_exfat_already
        call print_str
        jmp .chk_ntfs

.exec_load_exfat:
        push cs
        pop es
        mov dx, jload_path
        mov bx, exec_pb
        mov word [exec_cmd_off], cmd_load_exfat
        mov [exec_cmd_seg], cs
        mov word [exec_fcb1_off], 0x005C
        mov [exec_fcb1_seg], cs
        mov word [exec_fcb2_off], 0x006C
        mov [exec_fcb2_seg], cs
        call run_exec
        jc .load_exfat_fail

        ; Verify 0xFD02 is now registered
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD02
        int 0x2F
        mov ax, es
        or ax, di
        jz .load_exfat_fail
        push cs
        pop es
        mov si, msg_exfat_loaded
        call print_str
        jmp .chk_ntfs

.load_exfat_fail:
        push cs
        pop es
        mov si, msg_exfat_fail
        call print_str
        mov ax, 0x4C02
        int 0x21

.chk_ntfs:
        ; 3. NTFS (primary == 5 or secondary flag bit 5)
        cmp byte [fs_primary], 5
        je .do_ntfs
        test byte [fs_flags], 0x20
        jz .detect_done
.do_ntfs:
        mov byte [found_any], 1
        ; Check if NTFS.JLM (0xFD03) is already loaded
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD03
        int 0x2F
        mov ax, es
        or ax, di
        jz .exec_load_ntfs
        push cs
        pop es
        mov si, msg_ntfs_already
        call print_str
        jmp .detect_done

.exec_load_ntfs:
        push cs
        pop es
        mov dx, jload_path
        mov bx, exec_pb
        mov word [exec_cmd_off], cmd_load_ntfs
        mov [exec_cmd_seg], cs
        mov word [exec_fcb1_off], 0x005C
        mov [exec_fcb1_seg], cs
        mov word [exec_fcb2_off], 0x006C
        mov [exec_fcb2_seg], cs
        call run_exec
        jc .load_fail

        ; Verify 0xFD03 is now registered
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD03
        int 0x2F
        mov ax, es
        or ax, di
        jz .load_fail
        push cs
        pop es
        mov si, msg_ntfs_loaded
        call print_str
        jmp .detect_done

.load_fail:
        push cs
        pop es
        mov si, msg_load_fail
        call print_str
        mov ax, 0x4C02
        int 0x21

.detect_done:
        push cs
        pop es
        cmp byte [found_any], 0
        jne .detect_ok
        mov si, msg_unknown
        call print_str
.detect_ok:
        mov ax, 0x4C00
        int 0x21

.do_unload:
        mov byte [unloaded_any], 0

        ; Try unloading EXFAT.JLM (0xFD02) if present
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD02
        int 0x2F
        mov ax, es
        or ax, di
        jz .chk_unload_ntfs

        ; Unload safety: if XREDIR ('XRDR') is resident, detach its CDS slot
        ; and zero its jlm_entry BEFORE unloading EXFAT.JLM so INT 2Fh AX=11xx
        ; can never jump into a freed JLM vector (Exception 06).
        push cs
        pop es
        call detach_xredir
        cmp al, 2
        je .unload_xredir_busy

        push cs
        pop es
        mov dx, jload_path
        mov bx, exec_pb
        mov word [exec_cmd_off], cmd_unload_exfat
        mov [exec_cmd_seg], cs
        mov word [exec_fcb1_off], 0x005C
        mov [exec_fcb1_seg], cs
        mov word [exec_fcb2_off], 0x006C
        mov [exec_fcb2_seg], cs
        call run_exec
        jc .unload_fail
        mov byte [unloaded_any], 1
        mov si, msg_exfat_unloaded
        call print_str
        jmp .chk_unload_ntfs

.unload_xredir_busy:
        push cs
        pop es
        mov si, msg_xredir_busy
        call print_str
        mov ax, 0x4C04
        int 0x21

.chk_unload_ntfs:
        ; Try unloading NTFS.JLM (0xFD03) if present
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD03
        int 0x2F
        mov ax, es
        or ax, di
        jz .unload_done
        push cs
        pop es
        mov dx, jload_path
        mov bx, exec_pb
        mov word [exec_cmd_off], cmd_unload_ntfs
        mov [exec_cmd_seg], cs
        mov word [exec_fcb1_off], 0x005C
        mov [exec_fcb1_seg], cs
        mov word [exec_fcb2_off], 0x006C
        mov [exec_fcb2_seg], cs
        call run_exec
        jc .unload_fail
        mov byte [unloaded_any], 1
        mov si, msg_ntfs_unloaded
        call print_str

.unload_done:
        push cs
        pop es
        cmp byte [unloaded_any], 0
        jne .unload_ok
        mov si, msg_not_loaded
        call print_str
.unload_ok:
        mov ax, 0x4C00
        int 0x21

.unload_fail:
        push cs
        pop es
        mov si, msg_unload_fail
        call print_str
        mov ax, 0x4C03
        int 0x21

; Walk FreeDOS CDS array for a resident XREDIR ('XRDR') IFS entry.
; Returns AL = 0 (none), 1 (detached cleanly), or 2 (in use as current drive).
detach_xredir:
        mov ah, 0x19
        int 0x21
        mov [cur_drive], al
        mov ah, 0x52
        int 0x21
        mov ax, [es:bx + 0x16]
        mov [cds_off], ax
        mov ax, [es:bx + 0x18]
        mov [cds_seg], ax
        mov al, [es:bx + 0x21]
        mov [last_drive], al

        xor dl, dl
        movzx cx, byte [last_drive]
        les di, [cds_off]
.dx_scan:
        test cx, cx
        jz .dx_none
        test word [es:di + 67], 0xC000
        jz .dx_next
        push es
        push di
        les di, [es:di + 82]            ; CDS_IFS far pointer
        cmp dword [es:di], 0x52445258   ; 'XRDR'
        jne .dx_pop_next
        cmp dl, [cur_drive]
        jne .dx_clear
        pop di
        pop es
        push cs
        pop es
        mov al, 2
        ret
.dx_clear:
        cli
        mov dword [es:di + 8], 0        ; zero resident jlm_entry
        pop di
        pop es
        mov word [es:di + 67], 0        ; clear CDS_FLAGS
        sti
        push cs
        pop es
        mov si, msg_xredir_detached
        call print_str
        mov al, 1
        ret
.dx_pop_next:
        pop di
        pop es
.dx_next:
        add di, 88                      ; CDS_SIZE
        inc dl
        dec cx
        jmp .dx_scan
.dx_none:
        push cs
        pop es
        xor al, al
        ret

; Run DOS EXEC (INT 21h AX=4B00h) preserving SS:SP across DOS 2.x/FreeDOS quirks.
run_exec:
        mov [saved_ss], ss
        mov [saved_sp], sp
        mov ax, 0x4B00
        int 0x21
        cli
        mov ss, [cs:saved_ss]
        mov sp, [cs:saved_sp]
        sti
        push cs
        pop ds
        push cs
        pop es
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

core_off:       dw 0
core_seg:       dw 0
saved_ss:       dw 0
saved_sp:       dw 0
unloaded_any:   db 0
fs_primary:     db 0
fs_flags:       db 0
found_any:      db 0
cur_drive:      db 0
last_drive:     db 0
cds_off:        dw 0
cds_seg:        dw 0

exec_pb:
        dw 0                    ; inherit parent environment
exec_cmd_off:   dw 0
exec_cmd_seg:   dw 0
exec_fcb1_off:  dw 0
exec_fcb1_seg:  dw 0
exec_fcb2_off:  dw 0
exec_fcb2_seg:  dw 0

jload_path:       db 'JLOAD.EXE', 0
cmd_load_exfat:   db 10, ' EXFAT.JLM', 13
cmd_unload_exfat: db 13, ' -u EXFAT.JLM', 13
cmd_load_ntfs:    db 9, ' NTFS.JLM', 13
cmd_unload_ntfs:  db 12, ' -u NTFS.JLM', 13

msg_no_core:        db 'FSLOAD: ERROR FERRIC.JLM (0xFD01) not loaded', 13, 10, '$'
msg_unknown:        db 'FSLOAD: no supported USB filesystem detected', 13, 10, '$'
msg_fat:            db 'FSLOAD: FAT detected (core-resident)', 13, 10, '$'
msg_exfat_already:  db 'FSLOAD: EXFAT.JLM already loaded (0xFD02)', 13, 10, '$'
msg_exfat_loaded:   db 'FSLOAD: loaded EXFAT.JLM (0xFD02)', 13, 10, '$'
msg_exfat_fail:     db 'FSLOAD: ERROR failed to load EXFAT.JLM', 13, 10, '$'
msg_exfat_unloaded: db 'FSLOAD: unloaded EXFAT.JLM (0xFD02)', 13, 10, '$'
msg_xredir_detached: db 'FSLOAD: detached XREDIR drive', 13, 10, '$'
msg_xredir_busy:    db 'FSLOAD: ERROR XREDIR drive is current (switch to C: first)', 13, 10, '$'
msg_ntfs_already:   db 'FSLOAD: NTFS.JLM already loaded (0xFD03)', 13, 10, '$'
msg_ntfs_loaded:    db 'FSLOAD: loaded NTFS.JLM (0xFD03)', 13, 10, '$'
msg_load_fail:      db 'FSLOAD: ERROR failed to load NTFS.JLM', 13, 10, '$'
msg_not_loaded:     db 'FSLOAD: no loadable FS module is loaded', 13, 10, '$'
msg_ntfs_unloaded:  db 'FSLOAD: unloaded NTFS.JLM (0xFD03)', 13, 10, '$'
msg_unload_fail:    db 'FSLOAD: ERROR failed to unload FS module', 13, 10, '$'

        align 16
stack_base:
        times 512 db 0
stack_top:
