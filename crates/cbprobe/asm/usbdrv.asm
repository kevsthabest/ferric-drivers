; =============================================================================
; USBDRV.SYS - Gate 3: Writable DOS Block Device Driver for USB Mass Storage
; =============================================================================
; Loaded via CONFIG.SYS (`2?DEVICE=USBDRV.SYS`) to expose a USB flash drive
; (mounted by `USBPROBE`) as the next native DOS drive letter (e.g. D:).
;
; Strictly 386 baseline. Gate 3 enables writes ONLY via the qualified USB MSC
; backend in FERRIC.JLM (never BIOS INT 13h or ATA/IDE ports).
; Calls FERRIC.JLM V86 APIs:
;   AX=0x0015 -> fetch 25-byte DOS BPB + part_lba + media_gen
;   AX=0x0016 -> read one 512-byte sector at 32-bit LBA (DX:BX) into ES:DI
;   AX=0x0017 -> write one 512-byte sector at 32-bit LBA (DX:BX) from ES:DI
;   AX=0x0018 -> flush USB MSC write cache (SCSI SYNCHRONIZE CACHE(10))
;   AX=0x001A -> live media check (SCSI TEST UNIT READY + media_gen)
; =============================================================================

cpu 386
bits 16
org 0x0000

; --- DOS Block Device Header (18 bytes) ---
hdr_next:       dd 0xFFFFFFFF           ; last driver in chain
hdr_attr:       dw 0x2002               ; block device, non-IBM BPB (0x2000), 32-bit LBA (0x0002)
hdr_strat:      dw strategy
hdr_intr:       dw interrupt
hdr_units:      db 1                    ; 1 logical unit (D:)
hdr_name:       db 'USBDRV '            ; 7-byte pad

req_off:        dw 0
req_seg:        dw 0
jlm_off:        dw 0
jlm_seg:        dw 0
last_gen:       dw 0
do_verify:      db 0
                db 0
secs_done:      dw 0

bpb_ptr_tbl:    dw bpb_buf              ; array of 1 near BPB pointer for INIT

; 33-byte buffer filled by FERRIC.JLM AX=0x0015:
;   +00..+24: standard 25-byte DOS BPB (default 1.44M placeholder until USBPROBE)
;   +25..+28: part_lba (u32)
;   +29..+32: media_gen (u32)
bpb_buf:
        dw 512                          ; +00 bytes/sector
        db 1                            ; +02 sectors/cluster
        dw 1                            ; +03 reserved sectors
        db 2                            ; +05 number of FATs
        dw 224                          ; +06 root dir entries
        dw 2880                         ; +08 total sectors (16-bit)
        db 0xF8                         ; +0A media descriptor
        dw 9                            ; +0B sectors/FAT
        dw 63                           ; +0D sectors/track
        dw 16                           ; +0F heads
        dd 0                            ; +11 hidden sectors
        dd 0                            ; +15 total sectors (32-bit)
part_lba:
        dd 0                            ; +19 partition start LBA
media_gen:
        dd 0                            ; +1D mount generation

strategy:
        mov [cs:req_off], bx
        mov [cs:req_seg], es
        retf

interrupt:
        pushad
        push ds
        push es

        push cs
        pop ds

        les bx, [req_off]
        mov al, [es:bx + 2]             ; command code

        cmp al, 0
        je .cmd_init
        cmp al, 1
        je .cmd_media_check
        cmp al, 2
        je .cmd_build_bpb
        cmp al, 4
        je .cmd_read
        cmp al, 8
        je .cmd_write
        cmp al, 9
        je .cmd_write_verify

        ; Unknown / unsupported command
        mov word [es:bx + 3], 0x8103    ; Error: Unknown command
        jmp .ret_done

.cmd_init:
        mov byte [es:bx + 13], 1        ; 1 unit
        mov word [es:bx + 14], resident_end
        mov [es:bx + 16], cs            ; resident end CS:OFFSET
        mov word [es:bx + 18], bpb_ptr_tbl
        mov [es:bx + 20], cs            ; BPB pointer table CS:OFFSET
        mov word [es:bx + 3], 0x0100    ; Done
        jmp .ret_done

.cmd_media_check:
        call find_jlm
        jc .mc_not_ready
        push es
        push bx
        mov ax, 0x001A                  ; live SCSI TEST UNIT READY + media_gen
        call far [jlm_off]
        pop bx
        pop es
        test ax, ax
        jz .mc_not_ready
        cmp ax, 2                       ; Unit Attention media change detected
        je .mc_changed
        cmp word [last_gen], 0
        je .mc_changed
        cmp dx, [last_gen]
        jne .mc_changed
        mov byte [es:bx + 14], 1        ; media unchanged
        mov word [es:bx + 3], 0x0100
        jmp .ret_done
.mc_changed:
        mov byte [es:bx + 14], 0xFF     ; -1: media changed -> DOS will call BUILD BPB
        mov word [es:bx + 3], 0x0100
        jmp .ret_done
.mc_not_ready:
        mov word [last_gen], 0
        mov byte [es:bx + 14], 0xFF     ; -1: media removed / ejected
        mov word [es:bx + 3], 0x8102    ; Error: Drive not ready
        jmp .ret_done

.cmd_build_bpb:
        call find_jlm
        jc .err_not_ready
        push es
        push bx
        push cs
        pop es
        mov di, bpb_buf
        mov ax, 0x0015
        call far [jlm_off]
        pop bx
        pop es
        test ax, ax
        jz .err_not_ready
        mov [last_gen], dx
        mov word [es:bx + 18], bpb_buf
        mov [es:bx + 20], cs
        mov word [es:bx + 3], 0x0100
        jmp .ret_done

.cmd_read:
        call find_jlm
        jc .err_not_ready
        cmp word [last_gen], 0
        je .err_not_ready
        ; Ensure BPB/part_lba is current
        mov cx, [es:bx + 18]            ; sector count
        test cx, cx
        jz .read_ok

        ; Compute 32-bit LBA = part_lba + start_sector (reject 32-bit wrap)
        movzx eax, word [es:bx + 20]
        cmp ax, 0xFFFF
        jne .lba_ready
        mov eax, [es:bx + 26]           ; 32-bit starting sector (LBA)
.lba_ready:
        add eax, [part_lba]
        jc .err_not_ready

        ; Transfer destination ES:DI -> convert to normalized seg:off
        mov di, [es:bx + 14]
        mov si, [es:bx + 16]            ; dest segment

        push es
        push bx
        mov bp, cx                      ; remaining sector count
.read_loop:
        ; Normalize SI:DI so DI < 16 (prevents 64 KiB offset wrap across multi-sector reads)
        mov dx, di
        shr dx, 4
        add si, dx
        and di, 0x000F

        push eax
        push si
        push di
        push bp
        mov es, si
        mov bx, ax                      ; LBA lo
        mov edx, eax
        shr edx, 16                     ; LBA hi
        mov ax, 0x0016
        call far [jlm_off]
        mov cx, ax                      ; save JLM status before restoring LBA in EAX
        pop bp
        pop di
        pop si
        pop eax
        test cx, cx
        jz .read_fail

        inc eax                         ; next LBA
        add si, 32                      ; advance destination by 512 bytes (32 paras)
        dec bp
        jnz .read_loop

        pop bx
        pop es
.read_ok:
        mov word [es:bx + 3], 0x0100    ; Done
        jmp .ret_done

.read_fail:
        pop bx
        pop es
        mov word [es:bx + 18], 0        ; 0 sectors transferred
        mov word [es:bx + 3], 0x810B    ; Error: Read fault
        jmp .ret_done

.cmd_write_verify:
        mov byte [do_verify], 1
        jmp .cmd_write_common

.cmd_write:
        mov byte [do_verify], 0

.cmd_write_common:
        call find_jlm
        jc .err_not_ready
        cmp word [last_gen], 0
        je .err_not_ready
        mov cx, [es:bx + 18]            ; sector count
        mov word [secs_done], 0
        test cx, cx
        jz .write_ok

        ; Compute 32-bit LBA = part_lba + start_sector (reject 32-bit wrap)
        movzx eax, word [es:bx + 20]
        cmp ax, 0xFFFF
        jne .wlba_ready
        mov eax, [es:bx + 26]           ; 32-bit starting sector (LBA)
.wlba_ready:
        add eax, [part_lba]
        jc .err_not_ready

        ; Transfer source ES:DI -> normalized SI:DI
        mov di, [es:bx + 14]
        mov si, [es:bx + 16]

        push es
        push bx
        mov bp, cx                      ; remaining sector count
.write_loop:
        ; Normalize SI:DI so DI < 16 (prevents 64 KiB offset wrap)
        mov dx, di
        shr dx, 4
        add si, dx
        and di, 0x000F

        push eax
        push si
        push di
        push bp
        mov es, si
        mov bx, ax                      ; LBA lo
        mov edx, eax
        shr edx, 16                     ; LBA hi
        mov ax, 0x0017                  ; Gate 3 USB MSC SCSI WRITE(10)
        call far [jlm_off]
        mov cx, ax                      ; save JLM status before restoring LBA in EAX
        pop bp
        pop di
        pop si
        pop eax
        test cx, cx
        jz .write_fail

        cmp byte [do_verify], 0
        je .write_next

        ; Command 0x09 (Write-with-Verify): read sector back and compare 512 bytes
        push eax
        push si
        push di
        push bp
        push cs
        pop es
        mov di, verify_buf
        mov bx, ax                      ; LBA lo
        mov edx, eax
        shr edx, 16                     ; LBA hi
        mov ax, 0x0016                  ; SCSI READ(10)
        call far [jlm_off]
        mov cx, ax                      ; save JLM status before restoring LBA in EAX
        pop bp
        pop di
        pop si
        pop eax
        test cx, cx
        jz .write_fail

        push ds
        push es
        push si
        push di
        mov es, si                      ; ES:DI = caller's 512-byte source sector
        push cs
        pop ds
        mov si, verify_buf              ; DS:SI = read-back 512-byte sector
        mov cx, 128                     ; 128 dwords = 512 bytes
        cld
        repe cmpsd
        pop di
        pop si
        pop es
        pop ds
        jne .write_fail

.write_next:
        inc word [secs_done]
        inc eax                         ; next LBA
        add si, 32                      ; advance source by 512 bytes (32 paras)
        dec bp
        jnz .write_loop

        ; Flush USB device write cache (SCSI SYNCHRONIZE CACHE(10))
        mov ax, 0x0018
        call far [jlm_off]
        test ax, ax
        jz .write_fail

        pop bx
        pop es
.write_ok:
        mov word [es:bx + 3], 0x0100    ; Done
        jmp .ret_done

.write_fail:
        pop bx
        pop es
        mov ax, [secs_done]
        mov [es:bx + 18], ax            ; sectors written before fault
        mov word [es:bx + 3], 0x810A    ; Error: Write fault
        jmp .ret_done

.err_not_ready:
        mov word [es:bx + 3], 0x8102    ; Error: Drive not ready

.ret_done:
        pop es
        pop ds
        popad
        retf

; Discover FERRIC.JLM lazily (INT 2Fh AX=1684h BX=0xFD01). CF=0 if present.
find_jlm:
        mov ax, [jlm_seg]
        or ax, [jlm_off]
        jnz .fj_ok
        push es
        push bx
        push di
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD01
        int 0x2F
        mov [jlm_off], di
        mov ax, es
        mov [jlm_seg], ax
        pop di
        pop bx
        pop es
        or ax, [jlm_off]
        jz .fj_fail
.fj_ok:
        clc
        ret
.fj_fail:
        stc
        ret

verify_buf:     times 512 db 0
resident_end:
