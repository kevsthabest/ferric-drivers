; =============================================================================
; SF2LOAD.COM - ALi M5451 Hardware Wavetable SoundFont (.SF2) Loader (XMS 3.0)
; (JLM verbs 0x0040..0x0044 in FERRIC.JLM + XMS 3.0 locked extended memory)
; =============================================================================
; Usage:
;   SF2LOAD <FILE.SF2>   Load a SoundFont 2.0 (.SF2, up to 256 MiB) into locked
;                        XMS 3.0 RAM and activate it for MPU-401 MIDI (0x330)
;   SF2LOAD /S           Show currently loaded SoundFont bank status
;   SF2LOAD /U           Unload active SoundFont (revert to built-in 4 KiB ROM)
;                        and free its XMS memory block
;
; Strictly 386 baseline, read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

CHUNK_SIZE equ 16384

start:
        push cs
        pop ds
        push cs
        pop es

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

        ; Discover XMS driver (INT 2Fh AX=4300h -> AL=80h, AX=4310h -> ES:BX)
        mov ax, 0x4300
        int 0x2F
        cmp al, 0x80
        jne .no_xms_ok
        mov ax, 0x4310
        int 0x2F
        mov [xms_off], bx
        mov [xms_seg], es
        mov byte [has_xms], 1
        ; Query XMS version (AH=00h -> AX = BCD version, e.g. 0x0300 for XMS 3.0)
        xor ah, ah
        call far [xms_off]
        mov [xms_ver], ax
.no_xms_ok:
        push cs
        pop es

        ; Parse PSP command tail: skip leading spaces
        movzx cx, byte [0x0080]
        mov si, 0x0081
.skip_ws:
        test cx, cx
        jz .show_usage
        mov al, [si]
        cmp al, ' '
        je .ws_next
        cmp al, 9
        je .ws_next
        cmp al, 13
        je .show_usage
        jmp .got_arg
.ws_next:
        inc si
        dec cx
        jmp .skip_ws

.got_arg:
        cmp al, '/'
        je .parse_opt
        cmp al, '-'
        je .parse_opt

        ; Copy filename into fname_buf (NUL-terminated)
        mov di, fname_buf
        mov bx, 127
.copy_fn:
        test cx, cx
        jz .fn_done
        lodsb
        dec cx
        cmp al, ' '
        je .fn_done
        cmp al, 9
        je .fn_done
        cmp al, 13
        je .fn_done
        stosb
        dec bx
        jnz .copy_fn
.fn_done:
        xor al, al
        stosb
        jmp .do_load

.parse_opt:
        inc si
        dec cx
        test cx, cx
        jz .show_usage
        lodsb
        or al, 0x20
        cmp al, 's'
        je .do_status
        cmp al, 'u'
        je .do_unload
        jmp .show_usage

.show_usage:
        mov si, msg_usage
        call print_str
        mov ax, 0x4C00
        int 0x21

; -----------------------------------------------------------------------------
; /S : Status
; -----------------------------------------------------------------------------
.do_status:
        mov di, io_buf
        mov cx, 1024
        mov ax, 0x0044
        call far [jlm_off]
        push cs
        pop es
        call print_report
        mov ax, 0x4C00
        int 0x21

; -----------------------------------------------------------------------------
; /U : Unload and free XMS handle
; -----------------------------------------------------------------------------
.do_unload:
        mov ax, 0x0043
        call far [jlm_off]
        push cs
        pop es
        test ax, ax
        jz .unload_none
        call xms_free_handle
        mov si, msg_unloaded
        call print_str
        mov ax, 0x4C00
        int 0x21
.unload_none:
        mov si, msg_none
        call print_str
        mov ax, 0x4C00
        int 0x21

; -----------------------------------------------------------------------------
; Load <FILE.SF2> into locked XMS 3.0 RAM and commit to FERRIC.JLM
; -----------------------------------------------------------------------------
.do_load:
        cmp byte [has_xms], 0
        jne .xms_present
        mov si, msg_no_xms
        call print_str
        mov ax, 0x4C02
        int 0x21

.xms_present:
        ; Open file read-only (INT 21h AH=3Dh AL=00h)
        mov dx, fname_buf
        mov ax, 0x3D00
        int 0x21
        jnc .open_ok
        mov si, msg_open_err
        call print_str
        mov ax, 0x4C03
        int 0x21

.open_ok:
        mov [file_handle], ax

        ; Get 32-bit file size: seek to end (AX=4202h CX=0 DX=0) -> DX:AX
        mov bx, ax
        xor cx, cx
        xor dx, dx
        mov ax, 0x4202
        int 0x21
        jc .err_close_read
        mov [file_size], ax
        mov [file_size + 2], dx

        ; Seek back to offset 0 (AX=4200h)
        xor cx, cx
        xor dx, dx
        mov ax, 0x4200
        int 0x21
        jc .err_close_read

        ; Validate 64 B <= file_size <= 256 MiB (0x1000_0000)
        mov eax, [file_size]
        cmp eax, 64
        jb .err_bad_size
        cmp eax, 0x1000_0000
        ja .err_bad_size

        ; Allocate XMS block: kb = (file_size + 4095) >> 10
        add eax, 4095
        shr eax, 10
        mov edx, eax
        cmp word [xms_ver], 0x0300
        jb .alloc_xms2
        ; XMS 3.0 Allocate Any Extended Memory (AH=89h, EDX = size in KiB)
        mov ah, 0x89
        call far [xms_off]
        cmp ax, 1
        je .alloc_ok
        ; Fallback to XMS 2.0 if <= 65535 KiB
        cmp edx, 65535
        ja .err_xms_alloc
.alloc_xms2:
        cmp edx, 65535
        ja .err_xms_alloc
        mov ah, 0x09                    ; XMS 2.0 Allocate EMB (DX = KiB)
        call far [xms_off]
        cmp ax, 1
        jne .err_xms_alloc
.alloc_ok:
        mov [xms_handle], dx

        ; Lock XMS block -> 32-bit physical address in DX:BX
        mov ah, 0x0C                    ; XMS Lock EMB
        call far [xms_off]
        cmp ax, 1
        jne .err_xms_lock
        mov [phys_addr], bx
        mov [phys_addr + 2], dx

        ; Call JLM AX=0x0040 (gate1_sf2_begin): EBX=phys, ECX=size, DX=handle
        mov ebx, [phys_addr]
        mov ecx, [file_size]
        mov dx, [xms_handle]
        mov ax, 0x0040
        call far [jlm_off]
        push cs
        pop es
        test ax, ax
        jnz .err_jlm_begin

        ; If a previous XMS handle was returned in DX, free it cleanly
        test dx, dx
        jz .no_prev_xms
        mov ax, dx
        call xms_free_handle
.no_prev_xms:

        mov si, msg_loading
        call print_str

        ; Prepare XMS Move EMB struct (streaming directly into XMS with zero Ring-0 PTEs!)
        mov word [xmm_src_handle], 0
        mov word [xmm_src_off], chunk_buf
        mov [xmm_src_seg], cs
        mov ax, [xms_handle]
        mov [xmm_dst_handle], ax

        mov dword [cur_off], 0
        mov dword [dot_next], 2097152   ; print '.' every 2 MiB
.read_loop:
        mov bx, [file_handle]
        mov cx, CHUNK_SIZE
        mov dx, chunk_buf
        mov ah, 0x3F
        int 0x21
        jc .err_stream
        test ax, ax
        jz .stream_done

        movzx eax, ax                   ; EAX = bytes read
        mov [last_read], eax
        ; XMS Move requires an even byte length (chunk_buf has 2 pad bytes)
        test eax, 1
        jz .even_len
        inc eax
.even_len:
        mov [xmm_len], eax
        mov eax, [cur_off]
        mov [xmm_dst_off], eax

        mov si, xmm_struct
        mov ah, 0x0B                    ; XMS Move Extended Memory Block
        call far [xms_off]
        cmp ax, 1
        jne .err_stream

        mov eax, [last_read]
        add [cur_off], eax
        mov eax, [cur_off]
        cmp eax, [dot_next]
        jb .read_loop
        add dword [dot_next], 2097152
        mov al, '.'
        call print_char
        jmp .read_loop

.stream_done:
        ; Close file
        mov bx, [file_handle]
        mov ah, 0x3E
        int 0x21
        call print_crlf

        ; Commit & parse SF2 in Ring-0 (maps only INFO + pdta): AX=0x0042
        push cs
        pop es
        mov di, io_buf
        mov cx, 1024
        mov ax, 0x0042
        call far [jlm_off]
        push cs
        pop es
        push bx                         ; BX = 0 ok / 1 parse error
        call print_report
        pop bx
        test bx, bx
        jnz .err_parse
        mov ax, 0x4C00
        int 0x21

.err_parse:
        mov ax, 0x0043
        call far [jlm_off]
        push cs
        pop es
        test ax, ax
        jz .exit_fail
        call xms_free_handle
.exit_fail:
        mov ax, 0x4C07
        int 0x21

.err_stream:
        mov bx, [file_handle]
        mov ah, 0x3E
        int 0x21
        mov ax, 0x0043
        call far [jlm_off]
        push cs
        pop es
        test ax, ax
        jz .stream_msg
        call xms_free_handle
.stream_msg:
        call print_crlf
        mov si, msg_read_err
        call print_str
        mov ax, 0x4C06
        int 0x21

.err_jlm_begin:
        mov bx, [file_handle]
        mov ah, 0x3E
        int 0x21
        mov ax, [xms_handle]
        call xms_free_handle
        mov si, msg_jlm_err
        call print_str
        mov ax, 0x4C05
        int 0x21

.err_xms_lock:
        mov dx, [xms_handle]
        mov ah, 0x0A                    ; XMS Free EMB
        call far [xms_off]
.err_xms_alloc:
        mov bx, [file_handle]
        mov ah, 0x3E
        int 0x21
        mov si, msg_xms_err
        call print_str
        mov ax, 0x4C04
        int 0x21

.err_bad_size:
        mov bx, [file_handle]
        mov ah, 0x3E
        int 0x21
        mov si, msg_size_err
        call print_str
        mov ax, 0x4C03
        int 0x21

.err_close_read:
        mov bx, [file_handle]
        mov ah, 0x3E
        int 0x21
        mov si, msg_read_err
        call print_str
        mov ax, 0x4C03
        int 0x21

; Unlock + Free XMS handle in AX
xms_free_handle:
        cmp byte [has_xms], 0
        je .xf_done
        test ax, ax
        jz .xf_done
        push ax
        mov dx, ax
        mov ah, 0x0D                    ; XMS Unlock EMB
        call far [xms_off]
        pop dx
        mov ah, 0x0A                    ; XMS Free EMB
        call far [xms_off]
.xf_done:
        ret

print_report:
        mov cx, ax
        test cx, cx
        jz .pr_done
        mov si, io_buf
.pr_ch:
        lodsb
        call print_char
        loop .pr_ch
.pr_done:
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
        mov al, 13
        call print_char
        mov al, 10
        jmp print_char

jlm_off:      dw 0
jlm_seg:      dw 0
xms_off:      dw 0
xms_seg:      dw 0
xms_ver:      dw 0
has_xms:      db 0
file_handle:  dw 0
xms_handle:   dw 0
file_size:    dd 0
phys_addr:    dd 0
cur_off:      dd 0
last_read:    dd 0
dot_next:     dd 0

; XMS Extended Memory Move Structure (16 bytes, AH=0Bh)
xmm_struct:
xmm_len:        dd 0
xmm_src_handle: dw 0
xmm_src_off:    dw 0
xmm_src_seg:    dw 0
xmm_dst_handle: dw 0
xmm_dst_off:    dd 0

msg_no_jlm:   db 'SF2LOAD: FERRIC.JLM (0xFD01) not loaded', 13, 10, '$'
msg_no_xms:   db 'SF2LOAD: XMS driver not present (need JEMMEX/HIMEM)', 13, 10, '$'
msg_open_err: db 'SF2LOAD: cannot open SoundFont file', 13, 10, '$'
msg_size_err: db 'SF2LOAD: file size must be 64 B .. 256 MiB', 13, 10, '$'
msg_xms_err:  db 'SF2LOAD: XMS 3.0 allocation/lock failed (not enough extended RAM)', 13, 10, '$'
msg_jlm_err:  db 'SF2LOAD: FERRIC.JLM rejected XMS physical address', 13, 10, '$'
msg_read_err: db 'SF2LOAD: error reading/streaming SoundFont file', 13, 10, '$'
msg_loading:  db 'SF2LOAD: streaming into XMS 3.0$'
msg_unloaded: db 'SF2LOAD: SoundFont unloaded (reverted to built-in 4 KiB ROM).', 13, 10, '$'
msg_none:     db 'SF2LOAD: no SoundFont was loaded.', 13, 10, '$'
msg_usage:    db 'Ferric DOS ALi M5451 SoundFont Loader (SF2LOAD / XMS 3.0)', 13, 10
              db 'Usage:', 13, 10
              db '  SF2LOAD <FILE.SF2>  Load .SF2 bank (up to 256 MiB) into XMS 3.0', 13, 10
              db '  SF2LOAD /S          Show loaded SoundFont status', 13, 10
              db '  SF2LOAD /U          Unload SoundFont and free XMS', 13, 10, '$'

fname_buf:    times 128 db 0
io_buf:       times 1024 db 0
chunk_buf:    times (CHUNK_SIZE + 2) db 0
