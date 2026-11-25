; =============================================================================
; XREDIR.COM - Stage C: put the read-only exFAT USB volume on a DOS drive
; letter through the INT 2Fh AX=11xx redirector interface.
; =============================================================================
; Usage:
;   XREDIR [X:]          Claim drive X: (or the first free letter) for the
;                        exFAT volume the JLM has mounted from the USB stick.
;
; Requires FERRIC.JLM loaded and USBPROBE run first (the volume must be
; mounted before a letter can mean anything). The TSR owns no filesystem
; logic: it claims a CDS entry in IFS mode, hooks INT 2Fh, and for every
; AX=11xx call marshals the kernel's registers into a request block and
; far-calls the JLM (verb 0x0030), which answers from the block - or
; declines, in which case the original registers are restored and the
; call chains to the previous handler untouched. All other INT 2Fh
; traffic chains directly.
;
; The kernel contract this is built against is the vendored FreeDOS
; kernel's: CDS layout from hdr/cds.h (88-byte entries), the List of
; Lists from hdr/lol.h, the SDA pointer from INT 21h AX=5D06h.
; Strictly 386 baseline.
; =============================================================================

cpu 386
bits 16
org 0x0100

; Request block layout, shared with gate1_redir_call in the JLM.
REQ_FN          equ 0x00        ; in/out: full AX, then result AX
REQ_BX          equ 0x02
REQ_CX          equ 0x04
REQ_DX          equ 0x06
REQ_SI          equ 0x08
REQ_DI          equ 0x0A
REQ_DS          equ 0x0C
REQ_ES          equ 0x0E
REQ_OUT_FLAGS   equ 0x10        ; out: bit 0 = carry, bit 1 = consumed
REQ_SDA_SEG     equ 0x12
REQ_SDA_OFF     equ 0x14
REQ_CDS_SEG     equ 0x16
REQ_CDS_OFF     equ 0x18
REQ_DRIVE       equ 0x1A

OUT_CONSUMED    equ 0x0002
OUT_CARRY       equ 0x0001

; CDS field offsets (hdr/cds.h).
CDS_PATH        equ 0
CDS_FLAGS       equ 67
CDS_DPB         equ 69
CDS_REDIR_REC   equ 73
CDS_USER_DATA   equ 77
CDS_BACKSLASH   equ 79
CDS_IFS_TYPE    equ 81
CDS_IFS         equ 82
CDS_FS_DATA     equ 86
CDS_SIZE        equ 88

CDS_IFS_FLAGS   equ 0xC000      ; CDSNETWDRV | CDSPHYSDRV

; =============================================================================
; Resident part: INT 2Fh hook, request block, IFS header.
; =============================================================================

start:
        jmp install

int2f_handler:
        cmp ah, 0x11
        jne .chain
        ; If detached (XREDIR /U or FSLOAD /U), chain directly without calling
        ; a potentially unloaded EXFAT.JLM.
        cmp word [cs:jlm_entry + 2], 0
        je .chain
        ; Marshal the entry registers into the request block. Until DS
        ; is ours, every store goes through a CS override - DS here is
        ; the kernel's, and writing through it would corrupt DOS data.
        mov [cs:req_block + REQ_FN], ax
        mov [cs:req_block + REQ_BX], bx
        mov [cs:req_block + REQ_CX], cx
        mov [cs:req_block + REQ_DX], dx
        mov [cs:req_block + REQ_SI], si
        mov [cs:req_block + REQ_DI], di
        mov [cs:req_block + REQ_DS], ds
        mov [cs:req_block + REQ_ES], es
        push cs
        pop ds
        mov word [req_block + REQ_OUT_FLAGS], 0
        push cs
        pop es
        mov di, req_block
        mov ax, 0x0030                  ; gate1_redir_call
        call far [jlm_entry]
        test word [req_block + REQ_OUT_FLAGS], OUT_CONSUMED
        jz .chain_saved
        ; Consumed: the block now holds the result registers.
        mov ax, [req_block + REQ_FN]
        mov bx, [req_block + REQ_BX]
        mov cx, [req_block + REQ_CX]
        mov dx, [req_block + REQ_DX]
        mov si, [req_block + REQ_SI]
        mov di, [req_block + REQ_DI]
        mov es, [req_block + REQ_ES]
        push word [req_block + REQ_DS]
        pop ds
        ; Patch the carry flag into the stacked flags image: an INT
        ; handler cannot set the caller's flags by setting its own.
        push bp
        mov bp, sp
        test word [cs:req_block + REQ_OUT_FLAGS], OUT_CARRY
        jz .cf_clear
        or word [bp + 6], 0x0001
        jmp .cf_done
.cf_clear:
        and word [bp + 6], 0xFFFE
.cf_done:
        pop bp
        iret
.chain_saved:
        ; The JLM declined: restore the entry registers exactly and
        ; chain, so another redirector sees the call as if we were
        ; not here.
        mov ax, [req_block + REQ_FN]
        mov bx, [req_block + REQ_BX]
        mov cx, [req_block + REQ_CX]
        mov dx, [req_block + REQ_DX]
        mov si, [req_block + REQ_SI]
        mov di, [req_block + REQ_DI]
        mov es, [req_block + REQ_ES]
        push word [req_block + REQ_DS]
        pop ds
.chain:
        jmp far [cs:old_int2f]

; The IFS header the CDS points at. This kernel never dereferences it
; (cdsIfs is carried for other consumers), so its content is a signature
; plus the data a debugger wants first: the magic lets a second XREDIR
; install recognise that one is already resident.
ifs_header:
        dd 0x52445258                   ; 'XRDR'
        dd 0                            ; no next header
jlm_entry:
        dw 0, 0                         ; JLM V86 API entry (INT 2F 1684h)
old_int2f:
        dw 0, 0                         ; previous INT 2Fh handler

req_block:
        times 0x1C db 0

info_block:
        times 32 db 0

resident_end:

; =============================================================================
; Install part: runs once, then is discarded by the TSR exit.
; =============================================================================

install:
        push cs
        pop ds
        push cs
        pop es

        ; Check for /U or -U (unmount/detach resident XREDIR) before requiring EXFAT.JLM.
        movzx cx, byte [0x0080]
        mov si, 0x0081
.chk_u_scan:
        test cx, cx
        jz .do_install
        lodsb
        dec cx
        cmp al, ' '
        je .chk_u_scan
        cmp al, 9
        je .chk_u_scan
        cmp al, 13
        je .do_install
        cmp al, '/'
        je .chk_u_char
        cmp al, '-'
        jne .do_install
.chk_u_char:
        test cx, cx
        jz .do_install
        lodsb
        or al, 0x20
        cmp al, 'u'
        je .do_detach

.do_install:
        ; Discover EXFAT.JLM via INT 2Fh AX=1684h BX=0xFD02.
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD02
        int 0x2F
        mov [jlm_entry], di
        mov ax, es
        mov [jlm_entry + 2], ax
        or ax, di
        jnz .have_jlm
        mov si, msg_no_jlm
        call print_str
        mov ax, 0x4C01
        int 0x21
.have_jlm:
        push cs
        pop es

        ; The volume must already be mounted: a letter claimed for a
        ; stick that is not there would fail every call against it.
        mov di, info_block
        mov ax, 0x0031                  ; gate1_redir_query
        call far [jlm_entry]
        test ax, ax
        jnz .have_vol
        mov si, msg_no_vol
        call print_str
        mov ax, 0x4C02
        int 0x21
.have_vol:

        ; Parse the optional drive letter from the command tail.
        movzx cx, byte [0x0080]
        mov si, 0x0081
        mov byte [want_drive], 0xFF
.skip_ws:
        test cx, cx
        jz .parsed
        lodsb
        dec cx
        cmp al, ' '
        je .skip_ws
        cmp al, 9
        je .skip_ws
        cmp al, 13
        je .parsed
        ; First real character: a letter is a request, anything else
        ; is a usage error.
        or al, 0x20
        cmp al, 'a'
        jb .usage
        cmp al, 'z'
        ja .usage
        sub al, 'a'
        mov [want_drive], al
        jmp .parsed
.usage:
        mov si, msg_usage
        call print_str
        mov ax, 0x4C03
        int 0x21
.parsed:

        ; List of Lists: ES:BX, CDS array far pointer at +0x16, the
        ; LASTDRIVE count at +0x21 (hdr/lol.h).
        mov ah, 0x52
        int 0x21
        mov ax, [es:bx + 0x16]
        mov [cds_off], ax
        mov ax, [es:bx + 0x18]
        mov [cds_seg], ax
        mov al, [es:bx + 0x21]
        mov [last_drive], al
        push cs
        pop es

        ; Refuse a second install: walk the CDS array for an IFS entry
        ; whose header carries our signature.
        movzx cx, byte [last_drive]
        les di, [cds_off]
.scan:
        test cx, cx
        jz .scanned
        test word [es:di + CDS_FLAGS], CDS_IFS_FLAGS
        jz .scan_next
        push es
        push di
        les di, [es:di + CDS_IFS]
        cmp dword [es:di], 0x52445258
        pop di
        pop es
        je .already
.scan_next:
        add di, CDS_SIZE
        dec cx
        jmp .scan
.already:
        mov si, msg_already
        call print_str
        mov ax, 0x4C04
        int 0x21
.scanned:

        ; Choose the entry: the requested letter, or the first free
        ; one from C: up (A: and B: are never auto-claimed).
        les di, [cds_off]
        cmp byte [want_drive], 0xFF
        je .auto
        movzx ax, byte [want_drive]
        cmp al, [last_drive]
        jae .bad_drive
        imul ax, CDS_SIZE
        add di, ax
        test word [es:di + CDS_FLAGS], 0xFFFF
        jnz .busy
        mov al, [want_drive]
        mov [drive], al
        jmp .claim
.auto:
        mov al, 2
.auto_loop:
        cmp al, [last_drive]
        jae .no_free
        push ax
        movzx ax, al
        imul ax, CDS_SIZE
        add ax, [cds_off]
        mov di, ax
        pop ax
        test word [es:di + CDS_FLAGS], 0xFFFF
        jz .auto_found
        inc al
        jmp .auto_loop
.auto_found:
        mov [drive], al
        ; ES:DI already points at the chosen entry (base + index*88,
        ; with ES still the CDS segment from the les above).
        jmp .claim
.no_free:
        mov si, msg_no_free
        call print_str
        mov ax, 0x4C05
        int 0x21
.bad_drive:
        mov si, msg_bad_drive
        call print_str
        mov ax, 0x4C06
        int 0x21
.busy:
        mov si, msg_busy
        call print_str
        mov ax, 0x4C07
        int 0x21

.do_detach:
        ; Walk CDS array for resident XREDIR ('XRDR'), refuse if it is the
        ; current drive, otherwise zero its jlm_entry and release its CDS slot.
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
.det_scan:
        test cx, cx
        jz .det_none
        test word [es:di + CDS_FLAGS], CDS_IFS_FLAGS
        jz .det_next
        push es
        push di
        les di, [es:di + CDS_IFS]
        cmp dword [es:di], 0x52445258
        jne .det_pop_next
        cmp dl, [cur_drive]
        jne .det_clear
        pop di
        pop es
        push cs
        pop es
        mov si, msg_in_use
        call print_str
        mov ax, 0x4C08
        int 0x21
.det_clear:
        cli
        mov dword [es:di + 8], 0        ; zero resident jlm_entry
        pop di
        pop es
        mov word [es:di + CDS_FLAGS], 0 ; release DOS drive letter
        sti
        push cs
        pop es
        mov si, msg_detached_a
        call print_str
        mov al, dl
        add al, 'A'
        call print_char
        mov si, msg_ok_b
        call print_str
        mov ax, 0x4C00
        int 0x21
.det_pop_next:
        pop di
        pop es
.det_next:
        add di, CDS_SIZE
        inc dl
        dec cx
        jmp .det_scan
.det_none:
        push cs
        pop es
        mov si, msg_not_inst
        call print_str
        mov ax, 0x4C00
        int 0x21

.claim:
        ; Fill the CDS entry. The current path is the root; the kernel
        ; rewrites it itself on every successful chdir.
        mov al, [drive]
        add al, 'A'
        mov [es:di + CDS_PATH], al
        mov byte [es:di + CDS_PATH + 1], ':'
        mov byte [es:di + CDS_PATH + 2], '\'
        mov byte [es:di + CDS_PATH + 3], 0
        mov word [es:di + CDS_FLAGS], CDS_IFS_FLAGS
        mov dword [es:di + CDS_DPB], 0
        mov word [es:di + CDS_REDIR_REC], ifs_header
        mov [es:di + CDS_REDIR_REC + 2], cs
        mov word [es:di + CDS_USER_DATA], 0xFFFF
        mov word [es:di + CDS_BACKSLASH], 2
        mov byte [es:di + CDS_IFS_TYPE], 0
        mov word [es:di + CDS_IFS], ifs_header
        mov [es:di + CDS_IFS + 2], cs
        mov word [es:di + CDS_FS_DATA], 0
        ; Remember the entry's far pointer for the request block.
        mov [req_block + REQ_CDS_OFF], di
        mov ax, es
        mov [req_block + REQ_CDS_SEG], ax
        push cs
        pop es

        ; SDA pointer via INT 21h AX=5D06h -> DS:SI. The call returns
        ; with DS pointing at the kernel, so the pointer is captured
        ; into registers before DS is ours again; the request block
        ; then carries it on every call so the JLM never has to ask.
        mov ax, 0x5D06
        int 0x21
        mov bx, ds
        mov cx, si
        push cs
        pop ds
        mov [req_block + REQ_SDA_OFF], cx
        mov [req_block + REQ_SDA_SEG], bx

        mov al, [drive]
        mov [req_block + REQ_DRIVE], al

        ; Hook INT 2Fh.
        mov ax, 0x352F
        int 0x21
        mov [old_int2f], bx
        mov ax, es
        mov [old_int2f + 2], ax
        push cs
        pop es
        mov dx, int2f_handler
        mov ax, 0x252F
        int 0x21

        ; Report and go resident, keeping only the resident part.
        mov si, msg_ok_a
        call print_str
        mov al, [drive]
        add al, 'A'
        call print_char
        mov si, msg_ok_b
        call print_str
        mov dx, resident_end
        add dx, 15
        shr dx, 4
        mov ax, 0x3100
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

want_drive:   db 0xFF
drive:        db 0
cds_off:      dw 0
cds_seg:      dw 0
last_drive:   db 0
cur_drive:    db 0

msg_no_jlm:   db 'XREDIR: No exFAT volume mounted (EXFAT.JLM 0xFD02 not loaded - run FSLOAD)', 13, 10, '$'
msg_no_vol:   db 'XREDIR: No exFAT volume mounted (run USBPROBE first)', 13, 10, '$'
msg_usage:    db 'XREDIR: Usage: XREDIR [X: | /U]', 13, 10, '$'
msg_already:  db 'XREDIR: Already installed', 13, 10, '$'
msg_no_free:  db 'XREDIR: No free drive letter', 13, 10, '$'
msg_bad_drive: db 'XREDIR: Drive letter out of range', 13, 10, '$'
msg_busy:     db 'XREDIR: Drive letter already in use', 13, 10, '$'
msg_in_use:   db 'XREDIR: Cannot unmount current drive (switch to C: first)', 13, 10, '$'
msg_not_inst: db 'XREDIR: Not installed', 13, 10, '$'
msg_detached_a: db 'XREDIR: unmounted drive $'
msg_ok_a:     db 'XREDIR: exFAT volume mounted as $'
msg_ok_b:     db ':', 13, 10, '$'
