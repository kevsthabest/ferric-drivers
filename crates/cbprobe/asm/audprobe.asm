; =============================================================================
; AUDPROBE.COM - Gate 4a: ALi M5451 AC'97 Audio Bring-Up & Chime Diagnostic
; =============================================================================
; Usage:
;   AUDPROBE        Initialize ALi M5451, print codec/mixer info, play C5-E5-G5
;   AUDPROBE /SB    Start resident Sound Blaster Pro (A220 I5 D1) + AdLib OPL2
;                   emulation on ALi M5451, run live OPL2 FM + DSP self-test,
;                   and stay TSR holding the 16 KiB audio ring buffer
;   AUDPROBE /V 0..9 Set live volume (0=mute, 1=quiet .. 9=max, default 7)
;   AUDPROBE /D     Print live Sound Blaster / OPL2 telemetry (cmd, DMA, IRQs, acks)
;   AUDPROBE /E     Same as default with inverted AC'97 EAPD bit (0x26 bit 15 = 1)
;   AUDPROBE /S     Print current audio status without resetting hardware
;   AUDPROBE /X     Stop voices, remove SB/AdLib I/O traps, and mute cleanly
;
; Strictly 386 baseline, read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

AUDIO_DMA_PARAS equ 1024 ; 16 KiB

start:
        push cs
        pop ds
        push cs
        pop es
        mov [myseg], cs

        ; Shrink our COM allocation to 8 KiB (512 paras) so AH=48h can allocate
        ; the 16 KiB audio DMA buffer.
        mov sp, 0x1FFE
        mov bx, 512
        mov ah, 0x4A
        int 0x21

        ; Parse command-line tail at PSP 0x80
        mov cl, [0x0080]
        xor ch, ch
        mov si, 0x0081
.parse_loop:
        jcxz .parse_done
        lodsb
        dec cx
        cmp al, '/'
        je .parse_flag
        cmp al, '-'
        je .parse_flag
        jmp .parse_loop
.parse_flag:
        jcxz .parse_done
        lodsb
        dec cx
        and al, 0xDF            ; uppercase
        cmp al, 'E'
        jne .chk_s
        mov word [opt_flags], 1
        jmp .parse_loop
.chk_s:
        cmp al, 'S'
        jne .chk_x
        ; Check for /SB (Sound Blaster Pro + AdLib resident mode)
        jcxz .set_status
        mov al, [si]
        and al, 0xDF
        cmp al, 'B'
        jne .set_status
        inc si
        dec cx
        mov byte [opt_mode], 3
        jmp .parse_loop
.set_status:
        mov byte [opt_mode], 1
        jmp .parse_loop
.chk_x:
        cmp al, 'X'
        jne .chk_v
        mov byte [opt_mode], 2
        jmp .parse_loop
.chk_v:
        cmp al, 'V'
        jne .chk_d
        mov byte [opt_mode], 4
        ; Skip optional spaces/colon before volume digit 0..9
.v_skip:
        jcxz .parse_done
        mov al, [si]
        cmp al, ' '
        je .v_adv
        cmp al, ':'
        je .v_adv
        cmp al, '0'
        jb .parse_loop
        cmp al, '9'
        ja .parse_loop
        sub al, '0'
        xor ah, ah
        mov [opt_vol], ax
        inc si
        dec cx
        jmp .parse_loop
.v_adv:
        inc si
        dec cx
        jmp .v_skip
.chk_d:
        cmp al, 'D'
        jne .parse_loop
        mov byte [opt_mode], 5
        jmp .parse_loop
.parse_done:

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
        push cs
        pop es

        cmp byte [opt_mode], 5
        je .do_diag
        cmp byte [opt_mode], 4
        je .do_vol
        cmp byte [opt_mode], 3
        je pm_teardown.do_sb_start
        cmp byte [opt_mode], 2
        je .do_stop
        cmp byte [opt_mode], 1
        je .do_status

        ; Allocate 16 KiB DMA buffer
        mov bx, AUDIO_DMA_PARAS
        mov ah, 0x48
        int 0x21
        jnc .dma_ok
        mov si, msg_no_mem
        call print_str
        mov ax, 0x4C02
        int 0x21

.dma_ok:
        mov [dma_seg], ax

        ; Call AX=0x0020 (gate1_audio_init): BX=dma_seg, CX=flags, ES:DI=aud_buf
        push cs
        pop es
        mov di, aud_buf
        mov bx, [dma_seg]
        mov cx, [opt_flags]
        mov ax, 0x0020
        call far [jlm_off]
        mov [init_rc], ax

        push cs
        pop es
        call print_info

        cmp byte [aud_buf + 0x00], 0
        je .done_absent
        cmp word [init_rc], 0
        jne .done_fail

        ; Play 3-note chime: AX=0x0021, CX=flags, ES:DI=aud_buf
        mov si, msg_playing
        call print_str
        push cs
        pop es
        mov di, aud_buf
        mov cx, [opt_flags]
        mov ax, 0x0021
        call far [jlm_off]
        mov [chime_rc], ax

        push cs
        pop es
        mov si, msg_cso
        call print_str
        mov ax, [aud_buf + 0x14]
        call print_hex16
        mov si, msg_to
        call print_str
        mov ax, [aud_buf + 0x16]
        call print_hex16
        mov si, msg_changes
        call print_str
        mov eax, [aud_buf + 0x18]
        call print_dec32
        call print_crlf

        cmp word [chime_rc], 0
        jne .chime_fail
        mov si, msg_ok
        call print_str
        mov ax, 0x4C00
        int 0x21

.chime_fail:
        mov si, msg_cso_stall
        call print_str
        mov ax, 0x4C03
        int 0x21

.done_absent:
        mov si, msg_absent
        call print_str
        mov ax, 0x4C00
        int 0x21

.done_fail:
        mov si, msg_init_fail
        call print_str
        mov ax, [init_rc]
        call print_dec16
        call print_crlf
        mov ax, 0x4C04
        int 0x21

.do_status:
        push cs
        pop es
        mov di, aud_buf
        mov ax, 0x0022
        call far [jlm_off]
        push cs
        pop es
        call print_info
        mov ax, 0x4C00
        int 0x21

.do_diag:
        push cs
        pop es
        mov di, sb_diag_buf
        mov ax, 0x0027
        call far [jlm_off]
        push cs
        pop es
        ; Line 1: AUDPROBE: SB cmd=0xXX rate=RRRRR buf=BBBB blk=KKKK phys=0xXXXXXXXX auto=A st=S
        mov si, msg_d_cmd
        call print_str
        mov al, [sb_diag_buf + 0x00]
        call print_hex8
        mov si, msg_d_rate
        call print_str
        mov eax, [sb_diag_buf + 0x04]
        call print_dec32
        mov si, msg_d_buf
        call print_str
        mov eax, [sb_diag_buf + 0x0C]
        call print_dec32
        mov si, msg_d_blk
        call print_str
        mov eax, [sb_diag_buf + 0x10]
        call print_dec32
        mov si, msg_d_phys
        call print_str
        mov eax, [sb_diag_buf + 0x08]
        call print_hex32
        mov si, msg_d_auto
        call print_str
        mov al, [sb_diag_buf + 0x01]
        add al, '0'
        call print_char
        mov si, msg_d_st
        call print_str
        mov al, [sb_diag_buf + 0x02]
        add al, '0'
        call print_char
        call print_crlf
        ; Line 2: AUDPROBE: SB io=NNNN opl=MMMM irqs=IIII ack22e=AAAA active=X
        mov si, msg_d_io
        call print_str
        mov eax, [sb_diag_buf + 0x14]
        call print_dec32
        mov si, msg_d_opl
        call print_str
        mov eax, [sb_diag_buf + 0x18]
        call print_dec32
        mov si, msg_d_irqs
        call print_str
        mov ax, [sb_diag_buf + 0x1C]
        call print_dec16
        mov si, msg_d_ack
        call print_str
        mov ax, [sb_diag_buf + 0x1E]
        call print_dec16
        mov si, msg_d_act
        call print_str
        mov al, [sb_diag_buf + 0x03]
        add al, '0'
        call print_char
        call print_crlf
        ; Line 3: AUDPROBE: IRQ11 count=NNNN mode=86Hz-IRQ / 18Hz-POLL
        ; Live values from the AX=0x0022 status snapshot: a frozen or zero
        ; count with mode 18Hz-POLL means the stream is on the bursty
        ; timer fallback, which is the first thing to check when effects
        ; echo or repeat.
        push cs
        pop es
        mov di, aud_buf
        mov ax, 0x0022
        call far [jlm_off]
        push cs
        pop es
        mov si, msg_d_irq11
        call print_str
        mov eax, [aud_buf + 0x20]
        call print_dec32
        mov si, msg_d_mode
        call print_str
        cmp byte [aud_buf + 0x24], 0
        jne .d_mode_poll
        mov si, msg_d_mode1
        jmp .d_mode_print
.d_mode_poll:
        mov si, msg_d_mode2
.d_mode_print:
        call print_str
        call print_crlf
        ; Line 4: AUDPROBE: DMA rd=... wr=... pos=... mode=... pst=... nz=... tc=... sh=...
        push cs
        pop es
        mov di, aud_buf
        mov ax, 0x0028
        call far [jlm_off]
        push cs
        pop es
        mov si, msg_dma_rd
        call print_str
        mov eax, [aud_buf + 0x00]
        call print_dec32
        mov si, msg_dma_wr
        call print_str
        mov eax, [aud_buf + 0x04]
        call print_dec32
        mov si, msg_dma_pos
        call print_str
        mov ax, [aud_buf + 0x08]
        call print_dec16
        mov si, msg_dma_mode
        call print_str
        mov al, [aud_buf + 0x0A]
        call print_hex8
        mov si, msg_dma_pst
        call print_str
        mov al, [aud_buf + 0x0B]
        add al, '0'
        call print_char
        mov si, msg_dma_nz
        call print_str
        mov ax, [aud_buf + 0x0C]
        call print_dec16
        mov si, msg_dma_tc
        call print_str
        movzx ax, byte [aud_buf + 0x0E]
        call print_dec16
        mov si, msg_dma_rd3
        call print_str
        mov eax, [aud_buf + 0x10]
        call print_dec32
        call print_crlf
        ; Line 5: AUDPROBE: BUF b0=XXXXXXXXXXXX b1=XXXXXXXXXXXX
        mov si, msg_dma_b0
        call print_str
        mov bx, aud_buf + 0x14
        mov cx, 6
.d_b0_lp:
        mov al, [bx]
        call print_hex8
        inc bx
        loop .d_b0_lp
        mov si, msg_dma_b1
        call print_str
        mov bx, aud_buf + 0x1A
        mov cx, 6
.d_b1_lp:
        mov al, [bx]
        call print_hex8
        inc bx
        loop .d_b1_lp
        call print_crlf
        mov ax, 0x4C00
        int 0x21

.do_vol:
        mov bx, [opt_vol]
        mov ax, 0x0025
        call far [jlm_off]
        test ax, ax
        jnz .vol_not_init
        mov si, msg_vol_ok
        call print_str
        mov ax, [opt_vol]
        call print_dec16
        call print_crlf
        mov ax, 0x4C00
        int 0x21
.vol_not_init:
        mov si, msg_vol_err
        call print_str
        mov ax, 0x4C05
        int 0x21

.do_stop:
        ; When a resident /SB published its PM32 state, stopping has a
        ; protected-mode half (HDPMI traps + the PM INT 08h hook) that
        ; only a DPMI client can undo: pm_teardown performs the whole
        ; stop and exits from protected mode. It returns here only when
        ; no DPMI host is present, in which case the PM32 state died
        ; with the host and the plain real-mode stop below is all that
        ; remains. The resident is identified by the BDA block at
        ; 0040:00F8 (magic 'AP' at 00FA) plus its title string.
        push es
        mov ax, 0x0040
        mov es, ax
        cmp word [es:0x00FA], 0x4150
        jne .stop_check_done
        mov ax, [es:0x00F8]
        mov [res_seg], ax
        mov es, ax
        mov eax, [es:msg_title]
        cmp eax, [msg_title]
        jne .stop_check_done
        pop es
        call pm_teardown
        ; No DPMI host: clear the stale publication and report, then
        ; fall through to the real-mode stop.
        push es
        mov ax, 0x0040
        mov es, ax
        mov word [es:0x00F8], 0
        mov word [es:0x00FA], 0
        pop es
        mov si, msg_pm_gone
        call print_str
        jmp .stop_rm
.stop_check_done:
        pop es
.stop_rm:
        mov ax, 0x0023
        call far [jlm_off]
        mov si, msg_stopped
        call print_str
        mov ax, 0x4C00
        int 0x21

; -----------------------------------------------------------------------------
; /X protected-mode teardown. Entered in real mode with [res_seg] set to
; the resident /SB copy's segment. Copies the resident's saved PM state
; (trap handles, pre-hook PM INT 08h vector) into this copy, enters DPMI,
; verifies the live PM INT 08h vector really is the resident's hook,
; uninstalls the five HDPMI port traps by handle (vendor API AX=7,
; EDX=handle), restores the hooked vector, stops the JLM audio through a
; simulated real-mode call, clears the BDA publication block, prints, and
; exits from protected mode. Returns to the caller ONLY when no DPMI host
; is present -- with no host, the traps and the hook no longer exist.
; -----------------------------------------------------------------------------
pm_teardown:
        push es
        mov es, [res_seg]
        mov eax, [es:old_pm1c_off]
        mov [x_old1c_off], eax
        mov ax, [es:old_pm1c_seg]
        mov [x_old1c_seg], ax
        ; Restore real-mode IVT 0x73 (IRQ 11) before tearing down PM32/audio
        cmp word [es:old_rm73_seg], 0
        je .x_skip_rm73
        push ds
        mov dx, [es:old_rm73_off]
        mov ds, [es:old_rm73_seg]
        mov ax, 0x2573
        int 0x21
        pop ds
.x_skip_rm73:
        mov eax, [es:pm_trap_handles + 0]
        mov [x_handles + 0], eax
        mov eax, [es:pm_trap_handles + 4]
        mov [x_handles + 4], eax
        mov eax, [es:pm_trap_handles + 8]
        mov [x_handles + 8], eax
        mov eax, [es:pm_trap_handles + 12]
        mov [x_handles + 12], eax
        mov eax, [es:pm_trap_handles + 16]
        mov [x_handles + 16], eax
        mov eax, [es:pm_trap_handles + 20]
        mov [x_handles + 20], eax
        pop es

        ; Enter DPMI through the host entry point (same sequence /SB uses).
        mov ax, 0x1687
        int 0x2F
        test ax, ax
        jnz .pmt_ret            ; no DPMI host: nothing PM32 left to undo
        mov [dpmi_off], di
        mov [dpmi_seg], es
        test si, si
        jz .pmt_enter
        mov bx, si
        mov ah, 0x48
        int 0x21
        jc .pmt_ret
        mov es, ax
.pmt_enter:
        mov ax, 1               ; 32-bit DPMI client
        call far [dpmi_off]
        jc .pmt_ret

        ; --- Protected mode (16-bit default CS) from here on. ---
        ; Confirm the live PM INT 08h vector is the resident's hook --
        ; matching handler offset in a descriptor based at the resident
        ; segment -- before applying any saved state. Under a different
        ; host (or after another client replaced the vector) the saved
        ; handles and vector belong to a dead configuration and must
        ; not be applied.
        mov ax, 0x0204          ; Get PM Interrupt Vector 0x08 -> CX:EDX
        mov bl, 0x08
        int 0x31
        cmp edx, hdpmi_timer_hook
        jne .pmt_tail
        mov bx, cx
        mov ax, 0x0006          ; Get Segment Base Address -> CX:DX
        int 0x31
        jc .pmt_tail
        movzx eax, word [res_seg]
        shl eax, 4
        shl ecx, 16
        movzx edx, dx
        or edx, ecx
        cmp edx, eax
        jne .pmt_tail
        mov byte [x_pm_live], 1

        ; Uninstall the five HDPMI port traps by their saved handles.
        xor edi, edi
        mov es, di
        mov esi, str_hdpmi
        mov ax, 0x168A
        int 0x2F
        test al, al
        jnz .pmt_vec            ; not HDPMI: traps cannot be addressed,
                                ; still restore the vector below
        mov [hdpmi_off], edi
        mov [hdpmi_seg], es
        mov esi, x_handles
        mov cx, 6
.pmt_unloop:
        mov edx, [esi]
        add esi, 4
        test edx, edx
        jz .pmt_unnext
        mov eax, 7              ; HDPMI uninstall port trap, EDX = handle
        push cx
        push esi
        o32 call far [hdpmi_off]
        pop esi
        pop cx
.pmt_unnext:
        dec cx
        jnz .pmt_unloop

.pmt_vec:
        ; Restore the PM INT 08h vector the resident hooked.
        cmp word [x_old1c_seg], 0
        je .pmt_tail
        mov ax, 0x0205          ; Set PM Interrupt Vector 0x08 <- CX:EDX
        mov bl, 0x08
        mov cx, [x_old1c_seg]
        mov edx, [x_old1c_off]
        int 0x31

.pmt_tail:
        ; Stop the JLM audio (V86 API AX=0x0023) through a simulated
        ; real-mode far call, the same bridge the PM trap handler uses.
        mov dword [trap_rm_regs + 0x1C], 0x0023 ; EAX = verb
        mov dword [trap_rm_regs + 0x20], 0      ; flags
        mov dword [trap_rm_regs + 0x2E], 0      ; SS:SP = host stack
        mov ax, [jlm_off]
        mov [trap_rm_regs + 0x2A], ax           ; IP
        mov ax, [jlm_seg]
        mov [trap_rm_regs + 0x2C], ax           ; CS
        push ds
        pop es
        mov edi, trap_rm_regs
        xor ebx, ebx
        xor ecx, ecx
        mov ax, 0x0301
        int 0x31

        ; Clear the BDA publication block: no resident /SB remains.
        mov ax, 0x0002          ; DPMI Segment to Descriptor (0040h = BDA)
        mov bx, 0x0040
        int 0x31
        jc .pmt_prints
        mov es, ax
        mov word [es:0x00F8], 0
        mov word [es:0x00FA], 0

.pmt_prints:
        cmp byte [x_pm_live], 0
        je .pmt_print_stopped
        mov dx, msg_pm_removed
        call .pmt_print
.pmt_print_stopped:
        mov dx, msg_stopped_pm
        call .pmt_print
        mov ax, 0x4C00
        int 0x21

; Print a $-terminated string (DX = offset) through a simulated real-mode
; INT 21h AH=09h. Protected-mode helper for pm_teardown.
.pmt_print:
        mov word [rm_regs + 0x1C], 0x0900       ; EAX = 0900h
        mov [rm_regs + 0x14], dx                ; EDX = string offset
        mov ax, [myseg]
        mov [rm_regs + 0x24], ax                ; DS = myseg
        push ds
        pop es
        mov edi, rm_regs
        mov bl, 0x21
        xor bh, bh
        xor cx, cx
        mov ax, 0x0300
        int 0x31
        ret

.pmt_ret:
        ret

; -----------------------------------------------------------------------------
; /SB mode: Start resident Sound Blaster Pro (A220 I5 D1) + AdLib OPL2 (0x388)
; streaming on ALi M5451, verify trapped DSP + OPL2 ports, play a live OPL2 FM
; chord, and stay TSR holding the 16 KiB audio ring buffer.
; -----------------------------------------------------------------------------
.do_sb_start:
        ; Refuse a second resident copy: it would register a second,
        ; independent HDPMI trap set for the same ports and split SB
        ; IRQ delivery between two TSRs. A resident /SB publishes its
        ; segment in the BDA block at 0040:00F8 (magic 'AP' at 00FA);
        ; verify the magic AND the resident image's title string so a
        ; stale or foreign BDA value cannot block a legitimate start.
        push es
        mov ax, 0x0040
        mov es, ax
        cmp word [es:0x00FA], 0x4150
        jne .sb_not_resident
        mov ax, [es:0x00F8]
        mov es, ax
        mov eax, [es:msg_title]
        cmp eax, [msg_title]
        jne .sb_not_resident
        pop es
        mov si, msg_sb_already
        call print_str
        mov ax, 0x4C06
        int 0x21
.sb_not_resident:
        pop es
        mov bx, AUDIO_DMA_PARAS
        mov ah, 0x48
        int 0x21
        jnc .sb_dma_ok
        mov si, msg_no_mem
        call print_str
        mov ax, 0x4C02
        int 0x21

.sb_dma_ok:
        mov [dma_seg], ax
        ; Hook real-mode IVT 0x73 (IRQ 11) BEFORE starting M5451 IRQ 11 so
        ; HDPMI32i PM32->RM reflections land in rm_irq11_isr instead of the
        ; BIOS default stub (which masks IRQ 11 on slave PIC 0xA1).
        push es
        mov ax, 0x3573
        int 0x21
        mov [old_rm73_off], bx
        mov [old_rm73_seg], es
        pop es
        push ds
        push cs
        pop ds
        mov dx, rm_irq11_isr
        mov ax, 0x2573
        int 0x21
        pop ds
        push cs
        pop es
        mov di, aud_buf
        mov bx, [dma_seg]
        mov ax, 0x0024
        call far [jlm_off]
        mov [init_rc], ax
        push cs
        pop es
        call print_info
        cmp byte [aud_buf + 0x00], 0
        je .sb_unhook_absent
        cmp word [init_rc], 0
        jne .sb_unhook_fail
        jmp .sb_init_ok

.sb_unhook_absent:
        call .restore_rm73
        jmp start.done_absent

.sb_unhook_fail:
        call .restore_rm73
        jmp start.done_fail

; Restore real-mode IVT 0x73 (IRQ 11) if we hooked it before a non-resident exit.
.restore_rm73:
        cmp word [old_rm73_seg], 0
        je .r73_done
        push ds
        mov dx, [old_rm73_off]
        mov ds, [old_rm73_seg]
        mov ax, 0x2573
        int 0x21
        pop ds
        mov word [old_rm73_seg], 0
.r73_done:
        ret

.sb_init_ok:

        ; Verify SB Pro DSP reset (0x226) + version (0xE1 -> 3.02) via trapped I/O
        mov dx, 0x226
        mov al, 1
        out dx, al
        xor al, al
        out dx, al
        mov dx, 0x22A
        in al, dx
        mov [sb_rst_byte], al

        mov dx, 0x22C
        mov al, 0xE1
        out dx, al
        mov dx, 0x22A
        in al, dx
        mov [sb_ver_maj], al
        in al, dx
        mov [sb_ver_min], al

        ; Verify AdLib OPL2 detection handshake on 0x388/0x389
        mov al, 0x04
        mov bl, 0x60
        call opl_write
        mov al, 0x04
        mov bl, 0x80
        call opl_write
        mov dx, 0x388
        in al, dx
        and al, 0xE0
        mov [opl_s1], al
        mov al, 0x02
        mov bl, 0xFF
        call opl_write
        mov al, 0x04
        mov bl, 0x21
        call opl_write
        mov dx, 0x388
        in al, dx
        and al, 0xE0
        mov [opl_s2], al
        ; Clear timer flags
        mov al, 0x04
        mov bl, 0x80
        call opl_write

        ; Print SB Pro + AdLib live verification line
        mov si, msg_sb_ok
        call print_str
        mov al, [sb_ver_maj]
        call print_hex8
        mov al, '.'
        call print_char
        mov al, [sb_ver_min]
        call print_hex8
        mov si, msg_sb_rst
        call print_str
        mov al, [sb_rst_byte]
        call print_hex8
        mov si, msg_opl_det
        call print_str
        mov al, [opl_s1]
        call print_hex8
        mov al, '/'
        call print_char
        mov al, [opl_s2]
        call print_hex8
        call print_crlf

        ; Play a brief live AdLib OPL2 FM chord (C4+E4+G4) through trapped 0x388/0x389!
        mov si, msg_opl_play
        call print_str
        ; Program channels 0, 1, 2 with warm electric-piano FM patch
        mov al, 0x20
        mov bl, 0x01
        call opl_write
        mov al, 0x40
        mov bl, 0x14
        call opl_write
        mov al, 0x60
        mov bl, 0xF2
        call opl_write
        mov al, 0x80
        mov bl, 0x47
        call opl_write
        mov al, 0x23
        mov bl, 0x21
        call opl_write
        mov al, 0x43
        mov bl, 0x00
        call opl_write
        mov al, 0x63
        mov bl, 0xF1
        call opl_write
        mov al, 0x83
        mov bl, 0x27
        call opl_write
        mov al, 0xC0
        mov bl, 0x06
        call opl_write
        ; Channel 0 = C4 (fnum=0x157, block=4 -> B0=0x31)
        mov al, 0xA0
        mov bl, 0x57
        call opl_write
        mov al, 0xB0
        mov bl, 0x31
        call opl_write
        ; Wait ~6 BIOS timer ticks (~330 ms) so the ring buffer streams the FM note
        call wait_ticks6
        ; Key-off channel 0
        mov al, 0xB0
        mov bl, 0x11
        call opl_write
        call wait_ticks6

        mov si, msg_sb_tsr
        call print_str

        ; Also register protected-mode I/O port traps with HDPMI32i (if present)
        ; so 32-bit DPMI games like DOOM (DOS/4GW) hit the same FERRIC.JLM
        ; Sound Blaster Pro + AdLib OPL2 engine!
        mov ax, 0x1687
        int 0x2F
        test ax, ax
        jnz .sb_rm_tsr
        mov [dpmi_off], di
        mov [dpmi_seg], es
        test si, si
        jz .enter_dpmi_sb
        mov bx, si
        mov ah, 0x48
        int 0x21
        jc .sb_rm_tsr
        mov es, ax
.enter_dpmi_sb:
        mov ax, 1               ; 32-bit DPMI client
        call far [dpmi_off]
        jc .sb_rm_tsr

        ; Now in protected mode (16-bit default CS).
        ; Look up HDPMI32i vendor API ("HDPMI") via INT 2Fh AX=168Ah.
        mov byte [pm_stage], '1'
        xor edi, edi
        mov es, di
        mov esi, str_hdpmi
        mov ax, 0x168A
        int 0x2F
        test al, al
        jnz .pm_report_fail
        mov [hdpmi_off], edi
        mov [hdpmi_seg], es

        ; Allocate a 32-bit (D=1) code selector for hdpmi_sb_trap at myseg<<4
        mov byte [pm_stage], '2'
        mov ax, 0x0000
        mov cx, 1
        int 0x31
        jc .pm_report_fail
        mov [cs32_sel], ax
        mov bx, ax
        movzx eax, word [myseg]
        shl eax, 4
        mov dx, ax
        shr eax, 16
        mov cx, ax
        mov ax, 0x0007
        int 0x31
        jc .pm_report_fail
        mov bx, [cs32_sel]
        xor cx, cx
        mov dx, 0xFFFF
        mov ax, 0x0008
        int 0x31
        jc .pm_report_fail
        mov ax, cs
        and ax, 3
        shl ax, 5
        or ax, 0x409B           ; D=1 (32-bit), P=1, Code Readable, DPL=CPL
        mov cx, ax
        mov bx, [cs32_sel]
        mov ax, 0x0009
        int 0x31
        jc .pm_report_fail

        ; Pre-populate real-mode call struct CS:IP for FERRIC.JLM V86 API
        mov ax, [jlm_off]
        mov [trap_rm_regs + 0x2A], ax   ; IP
        mov ax, [jlm_seg]
        mov [trap_rm_regs + 0x2C], ax   ; CS

        ; Install HDPMI32i port trap for 0x220..0x22F (16 ports)
        mov byte [pm_stage], '3'
        mov esi, 0x220
        mov edi, 16
        movzx ecx, word [cs32_sel]
        xor ebx, ebx
        mov bx, ds
        mov edx, hdpmi_sb_trap
        mov eax, 6
        o32 call far [hdpmi_off]
        jc .pm_report_fail
        mov [pm_trap_handles + 0], eax  ; AX=7 uninstall needs the handle
                                        ; the install call returns in EAX

        ; Install HDPMI32i port trap for 0x388..0x38B (4 ports: OPL2 + OPL3 Bank 1)
        mov byte [pm_stage], '4'
        mov esi, 0x388
        mov edi, 4
        movzx ecx, word [cs32_sel]
        xor ebx, ebx
        mov bx, ds
        mov edx, hdpmi_sb_trap
        mov eax, 6
        o32 call far [hdpmi_off]
        jc .pm_report_fail
        mov [pm_trap_handles + 4], eax

        ; Also trap ISA DMA Channel 1 ports (0x02..0x03, 0x0A..0x0C, 0x83) in PM32
        ; so DOOM/DOS4GW DMA buffer base/page/count are shadowed accurately
        ; (DMA page port 0x83 is write-only on ALi M1533 hardware).
        mov byte [pm_stage], '5'
        mov esi, 0x02
        mov edi, 2
        movzx ecx, word [cs32_sel]
        xor ebx, ebx
        mov bx, ds
        mov edx, hdpmi_sb_trap
        mov eax, 6
        o32 call far [hdpmi_off]
        jc .pm_report_fail
        mov [pm_trap_handles + 8], eax

        mov esi, 0x0A
        mov edi, 3
        movzx ecx, word [cs32_sel]
        xor ebx, ebx
        mov bx, ds
        mov edx, hdpmi_sb_trap
        mov eax, 6
        o32 call far [hdpmi_off]
        jc .pm_report_fail
        mov [pm_trap_handles + 12], eax

        mov esi, 0x83
        mov edi, 1
        movzx ecx, word [cs32_sel]
        xor ebx, ebx
        mov bx, ds
        mov edx, hdpmi_sb_trap
        mov eax, 6
        o32 call far [hdpmi_off]
        jc .pm_report_fail
        mov [pm_trap_handles + 16], eax

        ; Install HDPMI32i port trap for 0x330..0x331 (2 ports: MPU-401 General MIDI)
        mov esi, 0x330
        mov edi, 2
        movzx ecx, word [cs32_sel]
        xor ebx, ebx
        mov bx, ds
        mov edx, hdpmi_sb_trap
        mov eax, 6
        o32 call far [hdpmi_off]
        jc .pm_report_fail
        mov [pm_trap_handles + 20], eax

        ; Hook PM32 INT 08h (PIT timer: 140 Hz in DOOM, 18.2 Hz at prompt) to
        ; deliver pending SB IRQ 5 cleanly in Protected Mode via BDA 0040:00F4
        ; counter without re-entering OPL2 writes!
        mov byte [pm_stage], '6'
        mov ax, 0x0002          ; DPMI Segment to Descriptor (0040h = BDA)
        mov bx, 0x0040
        int 0x31
        jc .pm_skip_1c
        mov [bda_sel], ax
        mov ax, 0x0204          ; Get PM Interrupt Vector 0x08 -> CX:EDX
        mov bl, 0x08
        int 0x31
        mov [old_pm1c_off], edx
        mov [old_pm1c_seg], cx
        mov ax, 0x0205          ; Set PM Interrupt Vector 0x08 <- CX:EDX
        mov bl, 0x08
        mov cx, [cs32_sel]
        mov edx, hdpmi_timer_hook
        int 0x31
.pm_skip_1c:

        ; Report PM32 traps installed via DPMI real-mode INT 21h AH=09h
        mov word [rm_regs + 0x1C], 0x0900       ; EAX = 0900h
        mov word [rm_regs + 0x14], msg_pm_ok    ; EDX = offset
        mov ax, [myseg]
        mov [rm_regs + 0x24], ax                ; DS = myseg
        push ds
        pop es
        mov edi, rm_regs
        mov bl, 0x21
        xor bh, bh
        xor cx, cx
        mov ax, 0x0300
        int 0x31
        jmp .pm_do_tsr

.pm_report_fail:
        mov al, [pm_stage]
        mov [msg_pm_fail_ch], al
        mov word [rm_regs + 0x1C], 0x0900
        mov word [rm_regs + 0x14], msg_pm_fail
        mov ax, [myseg]
        mov [rm_regs + 0x24], ax
        push ds
        pop es
        mov edi, rm_regs
        mov bl, 0x21
        xor bh, bh
        xor cx, cx
        mov ax, 0x0300
        int 0x31

.pm_do_tsr:
        ; Publish where this resident copy lives so a later AUDPROBE /X
        ; can find it and undo the PM32 half (HDPMI traps + PM INT 08h
        ; hook), and so a second /SB can refuse instead of registering a
        ; duplicate trap set. The block sits in the BIOS Data Area's
        ; inter-application area at 0040:00F8: this copy's segment word
        ; plus an 'AP' magic at 00FA. Consumers verify the magic and
        ; this image's title string before trusting the segment.
        mov ax, [bda_sel]
        test ax, ax
        jnz .pub_have_sel
        mov ax, 0x0002          ; DPMI Segment to Descriptor (0040h = BDA)
        mov bx, 0x0040
        int 0x31
        jc .pub_skip
        mov [bda_sel], ax
.pub_have_sel:
        push es
        mov es, ax
        mov ax, [myseg]
        mov [es:0x00F8], ax
        mov word [es:0x00FA], 0x4150
        pop es
.pub_skip:
        ; Stay resident from DPMI via real-mode INT 21h AX=3100h so HDPMI32i
        ; keeps our 32-bit trap selector AND the 16 KiB DMA buffer alive!
        mov word [rm_regs + 0x1C], 0x3100       ; AX = 3100h (TSR)
        mov word [rm_regs + 0x14], (audprobe_end - $$ + 0x100 + 15) / 16
        mov ax, [myseg]
        mov [rm_regs + 0x24], ax
        push ds
        pop es
        mov edi, rm_regs
        mov bl, 0x21
        xor bh, bh
        xor cx, cx
        mov ax, 0x0300
        int 0x31
        ; Fallback exit if TSR returned unexpectedly
        mov ax, 0x4C00
        int 0x21

.sb_rm_tsr:
        ; Real-mode fallback TSR (when DPMI/HDPMI32i is not loaded):
        ; publish CS in BDA 0040:00F8 so /X can unhook rm_irq11_isr, and retain
        ; the full image (rm_irq11_isr + old_rm73_* + jlm_off/jlm_seg).
        push es
        mov ax, 0x0040
        mov es, ax
        mov [es:0x00F8], cs
        mov word [es:0x00FA], 0x4150
        pop es
        mov ax, [0x002C]
        test ax, ax
        jz .sb_go_tsr
        mov es, ax
        mov ah, 0x49
        int 0x21
.sb_go_tsr:
        mov dx, (audprobe_end - $$ + 0x100 + 15) / 16
        mov ax, 0x3100
        int 0x21

; -----------------------------------------------------------------------------
; 32-bit Protected-Mode HDPMI32i I/O Port Trap Handler (called via cs32_sel)
; Entry: EDX=port, ECX=out (0=IN, 1=OUT), EAX=data, DS=our data selector.
; Exit:  EAX=read value (for IN), RETF.
; Calls FERRIC.JLM V86 API AX=0x0026 via DPMI INT 31h AX=0301h.
; -----------------------------------------------------------------------------
bits 32
align 4
hdpmi_sb_trap:
        push ebx
        push ecx
        push edx
        push edi
        push es
        mov dword [trap_rm_regs + 0x1C], 0x0026 ; EAX = 0x0026 (_v86_sb_io)
        mov [trap_rm_regs + 0x10], eax          ; EBX = data byte
        mov [trap_rm_regs + 0x18], ecx          ; ECX = out (0/1)
        mov [trap_rm_regs + 0x14], edx          ; EDX = port
        mov dword [trap_rm_regs + 0x20], 0      ; flags = 0
        mov dword [trap_rm_regs + 0x2E], 0      ; SP/SS = 0 (use host RM stack)
        push ds
        pop es
        mov edi, trap_rm_regs
        xor ebx, ebx
        xor ecx, ecx
        mov ax, 0x0301
        int 0x31
        movzx eax, word [trap_rm_regs + 0x1C]
        test ah, 1
        jz .trap_no_irq
        push eax
        int 0x0D                        ; deliver SB IRQ 5 in Protected Mode!
        pop eax
        and eax, 0xFF
.trap_no_irq:
        pop es
        pop edi
        pop edx
        pop ecx
        pop ebx
        retf

; -----------------------------------------------------------------------------
; 32-bit Protected-Mode INT 08h Hook (140 Hz in DOOM): delivers pending SB IRQ 5
; (INT 0Dh) at a clean interrupt boundary whenever FERRIC.JLM increments
; BDA 0040:00F4. Chains to the previous INT 08h handler cleanly.
; -----------------------------------------------------------------------------
align 4
hdpmi_timer_hook:
        push eax
        push es
        mov ax, [cs:bda_sel]
        mov es, ax
.pm1c_loop:
        cmp byte [es:0x00F4], 0
        je .pm1c_chain
        dec byte [es:0x00F4]
        int 0x0D
        jmp .pm1c_loop
.pm1c_chain:
        pop es
        pop eax
        jmp far dword [cs:old_pm1c_off]
bits 16

; Real-mode IVT 0x73 (IRQ 11) handler: when HDPMI32i reflects hardware IRQ 11
; from PM32 (DOOM) directly to real-mode IVT 0x73 (bypassing JEMM's V86 int
; chain), this services ALi M5451 via FERRIC.JLM AX=0x0029, avoids the BIOS
; default stub that masks IRQ 11 on 0xA1, and fires INT 0Dh at 86.1 Hz!
rm_irq11_isr:
        push ax
        push cx
        push dx
        mov ax, 0x0029
        call far [cs:jlm_off]
        test al, 1
        jz .rm73_chain
        mov cl, ah
        xor ch, ch
        jcxz .rm73_done
.rm73_sb_lp:
        int 0x0D
        loop .rm73_sb_lp
.rm73_done:
        pop dx
        pop cx
        pop ax
        iret
.rm73_chain:
        pop dx
        pop cx
        pop ax
        jmp far [cs:old_rm73_off]

; Write OPL2 register AL = BL via trapped ports 0x388/0x389
opl_write:
        push dx
        mov dx, 0x388
        out dx, al
        mov dx, 0x389
        mov al, bl
        out dx, al
        pop dx
        ret

; Wait 6 BIOS timer ticks (~330 ms) at BDA 0040:006C so _timer_hookproc streams audio
wait_ticks6:
        push ax
        push bx
        push cx
        push es
        mov ax, 0x0040
        mov es, ax
        mov cx, 6
.wt_loop:
        mov bx, [es:0x006C]
.wt_spin:
        hlt
        cmp bx, [es:0x006C]
        je .wt_spin
        loop .wt_loop
        pop es
        pop cx
        pop bx
        pop ax
        ret

; Print 40-byte AudioInfo in aud_buf
print_info:
        cmp byte [aud_buf + 0x00], 0
        je .pi_ret

        ; AUDPROBE: M5451 bdf=BB:DD.F rev=0xRR io=0xXXXX irq=III stage=S
        mov si, msg_bdf
        call print_str
        mov al, [aud_buf + 0x02]
        call print_hex8
        mov al, ':'
        call print_char
        mov al, [aud_buf + 0x03]
        shr al, 3
        call print_hex8
        mov al, '.'
        call print_char
        mov al, [aud_buf + 0x03]
        and al, 7
        add al, '0'
        call print_char

        mov si, msg_rev
        call print_str
        mov al, [aud_buf + 0x04]
        call print_hex8

        mov si, msg_io
        call print_str
        mov ax, [aud_buf + 0x08]
        call print_hex16

        mov si, msg_irq
        call print_str
        movzx ax, byte [aud_buf + 0x05]
        call print_dec16

        mov si, msg_stage
        call print_str
        movzx ax, byte [aud_buf + 0x01]
        call print_dec16
        call print_crlf

        ; AUDPROBE: codec=0xXXXXXXXX pwr=0xXXXX stimer=NNNN dma=0xXXXXXXXX eapd_inv=E
        mov si, msg_codec
        call print_str
        mov eax, [aud_buf + 0x0C]
        call print_hex32

        mov si, msg_pwr
        call print_str
        mov ax, [aud_buf + 0x0A]
        call print_hex16

        mov si, msg_stimer
        call print_str
        mov eax, [aud_buf + 0x10]
        call print_dec32

        mov si, msg_dma
        call print_str
        mov eax, [aud_buf + 0x1C]
        call print_hex32

        mov si, msg_eapd
        call print_str
        mov al, [aud_buf + 0x06]
        add al, '0'
        call print_char
        call print_crlf
.pi_ret:
        ret

; =============================================================================
; Console formatting helpers (DOS INT 21h AH=02h)
; =============================================================================
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
        push si
.ps_loop:
        lodsb
        test al, al
        jz .ps_done
        call print_char
        jmp .ps_loop
.ps_done:
        pop si
        pop ax
        ret

print_crlf:
        mov al, 13
        call print_char
        mov al, 10
        call print_char
        ret

print_hex4:
        and al, 0x0F
        add al, '0'
        cmp al, '9'
        jbe .ph4_out
        add al, 7
.ph4_out:
        jmp print_char

print_hex8:
        push ax
        shr al, 4
        call print_hex4
        pop ax
        jmp print_hex4

print_hex16:
        push ax
        mov al, ah
        call print_hex8
        pop ax
        jmp print_hex8

print_hex32:
        push eax
        shr eax, 16
        call print_hex16
        pop eax
        jmp print_hex16

print_dec16:
        movzx eax, ax
print_dec32:
        push eax
        push ebx
        push ecx
        push edx
        mov ebx, 10
        xor cx, cx
.pd_div:
        xor edx, edx
        div ebx
        push dx
        inc cx
        test eax, eax
        jnz .pd_div
.pd_out:
        pop ax
        add al, '0'
        call print_char
        loop .pd_out
        pop edx
        pop ecx
        pop ebx
        pop eax
        ret

; =============================================================================
; Data
; =============================================================================
msg_title:      db "=== Ferric DOS ALi M5451 Audio Probe (Gate 4a) ===", 13, 10, 0
msg_no_jlm:     db "AUDPROBE: ERROR FERRIC.JLM not loaded (boot Profile 2).", 13, 10, 0
msg_no_mem:     db "AUDPROBE: ERROR failed to allocate 16 KiB DMA buffer.", 13, 10, 0
msg_absent:     db "AUDPROBE: ALi M5451 (10B9:5451) not present on PCI bus.", 13, 10, 0
msg_bdf:        db "AUDPROBE: M5451 bdf=", 0
msg_rev:        db " rev=0x", 0
msg_io:         db " io=0x", 0
msg_irq:        db " irq=", 0
msg_stage:      db " stage=", 0
msg_codec:      db "AUDPROBE: codec=0x", 0
msg_pwr:        db " pwr=0x", 0
msg_stimer:     db " stimer=", 0
msg_dma:        db " dma=0x", 0
msg_eapd:       db " eapd_inv=", 0
msg_playing:    db "AUDPROBE: playing C5-E5-G5 chime on Voice 0 (48 kHz S16 stereo)...", 13, 10, 0
msg_cso:        db "AUDPROBE: DMA CSO 0x", 0
msg_to:         db " -> 0x", 0
msg_changes:    db " updates=", 0
msg_ok:         db "AUDPROBE: PASS (if silent, try AUDPROBE /E to toggle EAPD amp).", 13, 10, 0
msg_cso_stall:  db "AUDPROBE: WARN DMA CSO did not advance during playback.", 13, 10, 0
msg_init_fail:  db "AUDPROBE: FAIL init rc=", 0
msg_stopped:    db "AUDPROBE: audio stopped and muted.", 13, 10, 0
msg_sb_already: db "AUDPROBE: SB Pro already resident (run AUDPROBE /X first).", 13, 10, 0
msg_pm_gone:    db "AUDPROBE: no DPMI host; PM32 trap state already gone.", 13, 10, 0
msg_pm_removed: db "AUDPROBE: PM32 traps removed, PM INT 08h restored.", 13, 10, "$"
msg_stopped_pm: db "AUDPROBE: audio stopped and muted.", 13, 10, "$"
msg_sb_ok:      db "AUDPROBE: SB Pro DSP v", 0
msg_sb_rst:     db " rst=0x", 0
msg_opl_det:    db " OPL2=0x", 0
msg_opl_play:   db "AUDPROBE: playing live AdLib OPL2 FM note via port 0x388/0x389...", 13, 10, 0
msg_sb_tsr:     db "AUDPROBE: SB Pro (A220 I5 D1) + AdLib (0x388) RESIDENT.", 13, 10, 0
msg_pm_ok:      db "AUDPROBE: HDPMI32i PM32 traps installed (DOOM/DOS4GW ready).", 13, 10, "$"
msg_pm_fail:    db "AUDPROBE: WARN HDPMI32i PM32 trap failed at stage "
msg_pm_fail_ch: db "0.", 13, 10, "$"
msg_vol_ok:     db "AUDPROBE: volume set to ", 0
msg_vol_err:    db "AUDPROBE: ERROR audio not initialized (run AUDPROBE or AUDPROBE /SB first).", 13, 10, 0
msg_d_cmd:      db "AUDPROBE: SB cmd=0x", 0
msg_d_rate:     db " rate=", 0
msg_d_buf:      db " buf=", 0
msg_d_blk:      db " blk=", 0
msg_d_phys:     db " phys=0x", 0
msg_d_auto:     db " auto=", 0
msg_d_st:       db " st=", 0
msg_d_io:       db "AUDPROBE: SB io=", 0
msg_d_opl:      db " opl=", 0
msg_d_irqs:     db " irqs=", 0
msg_d_ack:      db " ack22e=", 0
msg_d_act:      db " active=", 0
msg_d_irq11:    db "AUDPROBE: IRQ11 count=", 0
msg_d_mode:     db " mode=", 0
msg_d_mode1:    db "86Hz-IRQ", 0
msg_d_mode2:    db "18Hz-POLL", 0
msg_dma_rd:     db "AUDPROBE: DMA rd=", 0
msg_dma_wr:     db " wr=", 0
msg_dma_pos:    db " pos=", 0
msg_dma_mode:   db " mode=0x", 0
msg_dma_pst:    db " pst=", 0
msg_dma_nz:     db " nz=", 0
msg_dma_tc:     db " tc=", 0
msg_dma_rd3:    db " rd3=", 0
msg_dma_b0:     db "AUDPROBE: BUF b0=", 0
msg_dma_b1:     db " b1=", 0
str_hdpmi:      db "HDPMI", 0

opt_mode:       db 0            ; 0=init+chime, 1=status, 2=stop, 3=sb-tsr, 4=vol, 5=diag
opt_vol:        dw 7
myseg:          dw 0
dpmi_off:       dw 0
dpmi_seg:       dw 0
cs32_sel:       dw 0
bda_sel:        dw 0
align 4
old_pm1c_off:   dd 0
old_pm1c_seg:   dw 0
                dw 0
old_rm73_off:   dw 0
old_rm73_seg:   dw 0
pm_jlm_off:     dd 0
pm_jlm_seg:     dw 0
                dw 0
hdpmi_off:      dd 0
hdpmi_seg:      dw 0
                dw 0
rm_regs:        times 50 db 0
trap_rm_regs:   times 50 db 0
pm_trap_handles: times 6 dd 0   ; handles returned by the six HDPMI AX=6 trap installs (AX=7 uninstall needs them)
res_seg:        dw 0            ; resident /SB segment, found through the BDA publication block
x_old1c_off:    dd 0            ; /X working copies of the resident's saved PM state
x_old1c_seg:    dw 0
x_pm_live:      db 0
x_handles:      times 6 dd 0
pm_stage:       db '0'
sb_rst_byte:    db 0
sb_ver_maj:     db 0
sb_ver_min:     db 0
opl_s1:         db 0
opl_s2:         db 0
opt_flags:      dw 0
jlm_off:        dw 0
jlm_seg:        dw 0
dma_seg:        dw 0
init_rc:        dw 0
chime_rc:       dw 0
aud_buf:        times 40 db 0
sb_diag_buf:    times 32 db 0
audprobe_end:
