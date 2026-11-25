; =============================================================================
; USBPROBE.COM - Gate 2.4: OHCI USB Host Controller, Root Hub & External Hub Probe
; =============================================================================
; Allocates a 16 KiB DOS DMA block, calls FERRIC.JLM V86 API AX=0x0010
; (gate1_usb_init) to initialize OHCI, power root hub + external USB hubs,
; and enumerate attached USB devices (hubs, HID mouse/keyboard, mass storage).
;
; Strictly 386 baseline, read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

USB_DMA_PARAS equ 1024 ; 16 KiB

start:
        push cs
        pop ds
        push cs
        pop es

        ; Shrink our own COM allocation to 8 KiB (512 paras) so AH=48h can
        ; allocate the 16 KiB OHCI DMA block.
        mov sp, 0x1FFE
        mov bx, 512
        mov ah, 0x4A
        int 0x21

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
        ; Allocate 16 KiB DMA block
        mov bx, USB_DMA_PARAS
        mov ah, 0x48
        int 0x21
        jnc .dma_ok

        mov si, msg_no_mem
        call print_str
        mov ax, 0x4C02
        int 0x21

.dma_ok:
        mov [dma_seg], ax

        ; Call AX=0x0010 (gate1_usb_init): BX=dma_seg, ES:DI=usb_buf
        push cs
        pop es
        mov di, usb_buf
        mov bx, [dma_seg]
        mov ax, 0x0010
        call far [jlm_off]

        push cs
        pop es

        cmp byte [usb_buf + 0x00], 0
        je .absent

        ; Line 1: USBPROBE: present=1 stage=S id=VVVV:DDDD bdf=BB:DD.F bar0=0xXXXXXXXX
        mov si, msg_p1
        call print_str
        movzx ax, byte [usb_buf + 0x01]
        call print_dec16

        mov si, msg_id
        call print_str
        mov ax, [usb_buf + 0x04]
        call print_hex16
        mov al, ':'
        call print_char
        mov ax, [usb_buf + 0x06]
        call print_hex16

        mov si, msg_bdf
        call print_str
        mov al, [usb_buf + 0x02]
        call print_hex8
        mov al, ':'
        call print_char
        mov al, [usb_buf + 0x03]
        shr al, 3
        call print_hex8
        mov al, '.'
        call print_char
        mov al, [usb_buf + 0x03]
        and al, 7
        add al, '0'
        call print_char

        mov si, msg_bar0
        call print_str
        mov eax, [usb_buf + 0x08]
        call print_hex32
        call print_crlf

        ; Line 2: USBPROBE: rev=0xXX ports=P devs=N hubs=H irq=I
        mov si, msg_rev
        call print_str
        mov al, [usb_buf + 0x0C]
        call print_hex8
        mov si, msg_ports
        call print_str
        movzx ax, byte [usb_buf + 0x10]
        call print_dec16
        mov si, msg_devs
        call print_str
        movzx ax, byte [usb_buf + 0x11]
        call print_dec16
        mov si, msg_hubs
        call print_str
        movzx ax, byte [usb_buf + 0x12]
        call print_dec16
        mov si, msg_irq
        call print_str
        movzx ax, byte [usb_buf + 0x13]
        call print_dec16
        call print_crlf

        ; Device list (usb_buf + 0x20, stride 24 bytes)
        movzx cx, byte [usb_buf + 0x11]
        test cx, cx
        jz .done
        mov bp, usb_buf + 0x20

.dev_loop:
        push cx
        mov si, msg_dev
        call print_str
        movzx ax, byte [bp + 0x00]      ; addr
        call print_dec16

        mov si, msg_par
        call print_str
        movzx ax, byte [bp + 0x01]      ; parent_addr (0 = root)
        call print_dec16
        mov al, ':'
        call print_char
        movzx ax, byte [bp + 0x02]      ; port
        call print_dec16

        mov si, msg_spd
        call print_str
        cmp byte [bp + 0x03], 0
        je .fs
        mov si, msg_ls
        jmp .spd_pr
.fs:
        mov si, msg_fs
.spd_pr:
        call print_str

        mov si, msg_id
        call print_str
        mov ax, [bp + 0x04]
        call print_hex16
        mov al, ':'
        call print_char
        mov ax, [bp + 0x06]
        call print_hex16

        mov si, msg_cls
        call print_str
        mov al, [bp + 0x08]
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [bp + 0x09]
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [bp + 0x0A]
        call print_hex8

        mov si, msg_if
        call print_str
        mov al, [bp + 0x0B]
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [bp + 0x0C]
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [bp + 0x0D]
        call print_hex8

        mov si, msg_ep
        call print_str
        movzx ax, byte [bp + 0x10]
        call print_dec16

        ; Human-readable class tag (check composite KBD+MOUSE via pad[0] at +0x15)
        cmp byte [bp + 0x08], 0x09
        je .tag_hub
        cmp byte [bp + 0x15], 3
        je .tag_km
        cmp byte [bp + 0x0B], 0x03
        jne .check_msc
        cmp byte [bp + 0x0D], 0x01
        je .tag_kbd
        cmp byte [bp + 0x0D], 0x02
        je .tag_mouse
        mov si, msg_tag_hid
        call print_str
        jmp .dev_next
.tag_km:
        mov si, msg_tag_km
        call print_str
        jmp .dev_next
.tag_hub:
        mov si, msg_tag_hub
        call print_str
        movzx ax, byte [bp + 0x0F]
        call print_dec16
        mov al, ']'
        call print_char
        jmp .dev_next
.tag_kbd:
        mov si, msg_tag_kbd
        call print_str
        jmp .dev_next
.tag_mouse:
        mov si, msg_tag_mouse
        call print_str
        jmp .dev_next
.check_msc:
        cmp byte [bp + 0x0B], 0x08
        jne .dev_next
        mov si, msg_tag_msc
        call print_str
.dev_next:
        call print_crlf
        add bp, 24
        pop cx
        dec cx
        jnz .dev_loop
        jmp .done

.absent:
        mov si, msg_absent
        call print_str

.done:
        ; Check usb_buf.reserved[0] (+0x18): bit 0 = mouse, bit 1 = kbd, bit 2 = msc.
        mov al, [usb_buf + 0x18]
        test al, 7
        jz .free_and_exit

        mov si, msg_hid_m
        call print_str
        mov al, [usb_buf + 0x18]
        and al, 1
        add al, '0'
        call print_char
        mov si, msg_hid_k
        call print_str
        mov al, [usb_buf + 0x18]
        shr al, 1
        and al, 1
        add al, '0'
        call print_char
        mov si, msg_hid_s
        call print_str
        mov al, [usb_buf + 0x18]
        shr al, 2
        and al, 1
        add al, '0'
        call print_char
        mov si, msg_hid_end
        call print_str

        ; Stage F (Step 1): when an MSC drive is present, query AX=0x0035
        ; and report the detected filesystem type and media generation.
        test byte [usb_buf + 0x18], 4
        jz .stay_tsr
        mov ax, 0x0035
        call far [jlm_off]
        push eax
        mov si, msg_fs_pfx
        call print_str
        pop eax
        push eax
        and ax, 0x00FF
        cmp ax, 5
        jbe .fs_idx_ok
        xor ax, ax
.fs_idx_ok:
        mov bx, ax
        shl bx, 1
        mov si, [fs_names + bx]
        call print_str
        ; Append secondary non-FAT partitions on multi-partition drives
        ; (AH bit 4 = BLK_FLAG_HAS_EXFAT, bit 5 = BLK_FLAG_HAS_NTFS).
        pop eax
        push eax
        test ah, 0x10
        jz .chk_sec_ntfs
        cmp al, 4
        je .chk_sec_ntfs
        mov si, msg_plus_exfat
        call print_str
.chk_sec_ntfs:
        pop eax
        push eax
        test ah, 0x20
        jz .fs_gen_print
        cmp al, 5
        je .fs_gen_print
        mov si, msg_plus_ntfs
        call print_str
.fs_gen_print:
        mov si, msg_fs_gen
        call print_str
        pop eax
        shr eax, 16
        call print_dec16
        call print_crlf

.stay_tsr:
        ; Stay TSR so the 16 KiB OHCI periodic DMA block remains owned and pinned.
        mov dx, usbprobe_end
        add dx, 15
        shr dx, 4
        mov ax, 0x3100
        int 0x21

.free_and_exit:
        mov ax, [dma_seg]
        test ax, ax
        jz .exit_ok
        mov es, ax
        mov ah, 0x49
        int 0x21
.exit_ok:
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

print_dec16:
        push ax
        push bx
        push cx
        push dx
        xor cx, cx
        mov bx, 10
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
dma_seg:    dw 0

msg_title:  db 'USBPROBE: Ferric DOS OHCI USB host & hub enumeration (Gate 2.4)', 13, 10, '$'
msg_no_jlm: db 'USBPROBE: ERROR FERRIC.JLM not loaded (boot Profile 1 or 2)', 13, 10, '$'
msg_no_mem: db 'USBPROBE: ERROR 16 KiB DMA allocation failed', 13, 10, '$'
msg_absent: db 'USBPROBE: present=0 stage=0', 13, 10, '$'
msg_p1:     db 'USBPROBE: present=1 stage=$'
msg_id:     db ' id=$'
msg_bdf:    db ' bdf=$'
msg_bar0:   db ' bar0=0x$'
msg_rev:    db 'USBPROBE: rev=0x$'
msg_ports:  db ' ports=$'
msg_devs:   db ' devs=$'
msg_hubs:   db ' hubs=$'
msg_irq:    db ' irq=$'
msg_dev:    db '  DEV addr=$'
msg_par:    db ' par=$'
msg_spd:    db ' spd=$'
msg_fs:     db 'FS$'
msg_ls:     db 'LS$'
msg_cls:    db ' cls=$'
msg_if:     db ' if=$'
msg_ep:     db ' ep=$'
msg_tag_hub:   db ' [HUB ports=$'
msg_tag_km:    db ' [KBD+MOUSE]$'
msg_tag_kbd:   db ' [KBD]$'
msg_tag_mouse: db ' [MOUSE]$'
msg_tag_hid:   db ' [HID]$'
msg_tag_msc:   db ' [MSC]$'
msg_hid_m:     db 'USBPROBE: active (mouse=$'
msg_hid_k:     db ' kbd=$'
msg_hid_s:     db ' msc=$'
msg_hid_end:   db ') - resident', 13, 10, '$'
msg_fs_pfx:    db 'USBPROBE: fs=$'
msg_fs_gen:    db ' gen=$'
fs_names:      dw fs_unk, fs_fat12, fs_fat16, fs_fat32, fs_exfat, fs_ntfs
fs_unk:        db 'UNKNOWN$'
fs_fat12:      db 'FAT12$'
fs_fat16:      db 'FAT16$'
fs_fat32:      db 'FAT32$'
fs_exfat:      db 'EXFAT$'
fs_ntfs:       db 'NTFS$'
msg_plus_exfat: db '+EXFAT$'
msg_plus_ntfs:  db '+NTFS$'
msg_crlf:   db 13, 10, '$'

usb_buf:    times 416 db 0
usbprobe_end:
