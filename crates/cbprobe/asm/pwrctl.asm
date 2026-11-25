; =============================================================================
; PWRCTL.COM - Gate 5a DynaBook SS 2000M Power Management Utility
; =============================================================================
; Pure 16-bit NASM DOS .COM utility providing:
;   PWRCTL STATUS                  - APM 1.2 status, VBE/PM 1.0 caps, idle state
;   PWRCTL BLANK                   - VBE/PM 1.0 display blank (any key wakes)
;   PWRCTL ON|STANDBY|SUSPEND|OFF  - VBE/PM 1.0 display power state (BH=0,1,2,4)
;   PWRCTL SLEEP [STANDBY|SUSPEND] - APM 1.2 system standby/suspend (INT 15h 5307h)
;   PWRCTL POWEROFF                - APM 1.2 system power-off (INT 15h 5307h CX=3)
;   PWRCTL IDLE [ON|OFF|STATUS]    - Safe CPU idle HLT TSR (INT 28h + INT 2Fh 1680h)
;
; Strictly 386 baseline, read-only with respect to storage.
; =============================================================================

cpu 386
bits 16
org 0x0100

PWRCTL_MUX_AX   equ 0xFD5A
PWRCTL_MUX_BX   equ 0x5057      ; 'PW'
PWRCTL_MUX_ACK  equ 0x5043      ; 'PC'

entry:
        jmp near start

; -----------------------------------------------------------------------------
; Tiny resident section (~120 bytes + 256-byte PSP = ~24 paragraphs)
; Kept resident only when `PWRCTL IDLE ON` installs the idle HLT hook.
; -----------------------------------------------------------------------------
old_int28:      dd 0
old_int2f:      dd 0
idle_enabled:   db 0
idle_count:     dw 0

; INT 28h (DOS Idle): called only when DOS is waiting at the prompt / console
; input, never inside hardware ISRs or tight audio/USB polling loops.
; STI before HLT guarantees IRQ 0 (PIT), IRQ 1 (kbd), IRQ 11 (PCI audio/USB/NIC),
; and IRQ 12 (PS/2 mouse) wake the CPU immediately.
isr_int28:
        pushf
        cmp byte [cs:idle_enabled], 0
        je .chain28
        inc word [cs:idle_count]
        sti
        hlt
.chain28:
        popf
        jmp far [cs:old_int28]

; INT 2Fh (Multiplex): handles PWRCTL private control (AX=FD5Ah, BX=5057h)
; and DPMI/Windows Release Current VM Time-Slice (AX=1680h).
isr_int2f:
        pushf
        cmp ax, PWRCTL_MUX_AX
        jne .chk_1680
        cmp bx, PWRCTL_MUX_BX
        jne .chk_1680
        cmp cl, 1
        jne .chk_disable
        mov byte [cs:idle_enabled], 1
        jmp .mux_reply
.chk_disable:
        cmp cl, 2
        jne .mux_reply
        mov byte [cs:idle_enabled], 0
.mux_reply:
        popf
        mov ax, PWRCTL_MUX_ACK
        mov bl, [cs:idle_enabled]
        mov cx, cs
        mov dx, [cs:idle_count]
        iret

.chk_1680:
        cmp ax, 0x1680
        jne .chain2f
        cmp byte [cs:idle_enabled], 0
        je .chain2f
        inc word [cs:idle_count]
        sti
        hlt
        popf
        xor al, al              ; AL=00h: time-slice release supported
        iret

.chain2f:
        popf
        jmp far [cs:old_int2f]

resident_end:

; -----------------------------------------------------------------------------
; Non-resident entry point & CLI dispatch
; -----------------------------------------------------------------------------
start:
        push cs
        pop ds
        push cs
        pop es

        ; Parse PSP command tail (0x80 = len, 0x81.. = chars)
        movzx cx, byte [0x80]
        mov si, 0x81
        call skip_ws
        test cx, cx
        jz .show_usage
        mov al, [si]
        cmp al, 0x0D
        je .show_usage
        cmp al, 0x00
        je .show_usage

        mov di, arg1_buf
        call read_upper_token
        call skip_ws
        test cx, cx
        jz .dispatch
        mov al, [si]
        cmp al, 0x0D
        je .dispatch
        cmp al, 0x00
        je .dispatch
        mov di, arg2_buf
        call read_upper_token

.dispatch:
        mov si, arg1_buf
        mov di, cmd_status
        call str_eq
        jc cmd_do_status

        mov si, arg1_buf
        mov di, cmd_blank
        call str_eq
        jc cmd_do_blank

        mov si, arg1_buf
        mov di, cmd_on
        call str_eq
        jc cmd_do_disp_on

        mov si, arg1_buf
        mov di, cmd_standby
        call str_eq
        jc cmd_do_disp_standby

        mov si, arg1_buf
        mov di, cmd_suspend
        call str_eq
        jc cmd_do_disp_suspend

        mov si, arg1_buf
        mov di, cmd_off
        call str_eq
        jc cmd_do_disp_off

        mov si, arg1_buf
        mov di, cmd_sleep
        call str_eq
        jc cmd_do_sleep

        mov si, arg1_buf
        mov di, cmd_poweroff
        call str_eq
        jc cmd_do_poweroff

        mov si, arg1_buf
        mov di, cmd_idle
        call str_eq
        jc cmd_do_idle

        mov si, arg1_buf
        mov di, cmd_battery
        call str_eq
        jc cmd_do_battery

        mov si, arg1_buf
        mov di, cmd_batt
        call str_eq
        jc cmd_do_battery

        mov si, arg1_buf
        mov di, cmd_throttle
        call str_eq
        jc cmd_do_throttle

        mov si, arg1_buf
        mov di, cmd_bright
        call str_eq
        jc cmd_do_bright

        mov si, arg1_buf
        mov di, cmd_help
        call str_eq
        jc .show_usage

        mov si, arg1_buf
        mov di, cmd_qmark
        call str_eq
        jc .show_usage

        mov si, arg1_buf
        mov di, cmd_slashq
        call str_eq
        jc .show_usage

        mov si, msg_bad_cmd
        call print_str
        mov si, msg_usage
        call print_str
        mov ax, 0x4C01
        int 0x21

.show_usage:
        mov si, msg_usage
        call print_str
        mov ax, 0x4C00
        int 0x21

; =============================================================================
; PWRCTL STATUS
; =============================================================================
cmd_do_status:
        mov si, msg_title
        call print_str

        ; --- 1. APM 1.2 probe ---
        call probe_apm_check
        cmp byte [apm_present], 0
        je .apm_absent

        mov si, msg_apm_ver
        call print_str
        mov ax, [apm_ver]
        call print_bcd_hi
        mov al, '.'
        call print_char
        mov ax, [apm_ver]
        xchg ah, al
        call print_bcd_hi
        mov si, msg_flags
        call print_str
        mov ax, [apm_flags]
        call print_hex16
        mov al, ')'
        call print_char
        call print_crlf

        ; Connect real-mode interface (5301h)
        call apm_connect_rm
        jc .apm_conn_err

        ; Query power status (530Ah BX=0001h)
        mov ax, 0x530A
        mov bx, 0x0001
        int 0x15
        jc .apm_stat_err
        mov [ac_status], bh
        mov [batt_status], bl
        mov [batt_pct], cl
        mov [batt_time], dx
        mov byte [apm_stat_ok], 1
        call apm_disconnect_if_ours

        ; AC line
        mov si, msg_ac
        call print_str
        mov al, [ac_status]
        cmp al, 0x01
        je .ac_online
        cmp al, 0x02
        je .ac_backup
        or al, al
        jz .ac_offline
        mov si, word_unknown
        jmp .ac_print
.ac_online:
        mov si, word_online
        jmp .ac_print
.ac_backup:
        mov si, word_backup
        jmp .ac_print
.ac_offline:
        mov si, word_offline
.ac_print:
        call print_str
        call print_crlf

        ; Battery state
        mov si, msg_batt
        call print_str
        mov al, [batt_status]
        cmp al, 0x00
        je .b_high
        cmp al, 0x01
        je .b_low
        cmp al, 0x02
        je .b_crit
        cmp al, 0x03
        je .b_chg
        cmp al, 0x04
        je .b_abs
        mov si, word_unknown
        jmp .b_print
.b_high:
        mov si, word_high
        jmp .b_print
.b_low:
        mov si, word_low
        jmp .b_print
.b_crit:
        mov si, word_critical
        jmp .b_print
.b_chg:
        mov si, word_charging
        jmp .b_print
.b_abs:
        mov si, word_absent
.b_print:
        call print_str
        call print_crlf

        ; Charge percentage
        mov si, msg_pct
        call print_str
        mov al, [batt_pct]
        cmp al, 0xFF
        je .pct_unk
        xor ah, ah
        call print_dec16
        mov al, '%'
        call print_char
        jmp .pct_done
.pct_unk:
        mov si, word_unknown
        call print_str
.pct_done:
        call print_crlf

        ; Remaining time (APM 1.2: bit 15 = 0 -> seconds, bit 15 = 1 -> minutes)
        mov si, msg_time
        call print_str
        mov ax, [batt_time]
        cmp ax, 0xFFFF
        je .time_unk
        test ax, 0x8000
        jnz .time_min
        call print_dec16
        mov si, word_seconds
        call print_str
        jmp .time_note
.time_min:
        and ax, 0x7FFF
        call print_dec16
        mov si, word_minutes
        call print_str
.time_note:
        ; Preserve known DynaBook SS 2000M anomaly note (34s on AC)
        cmp byte [ac_status], 0x01
        jne .time_est
        mov ax, [batt_time]
        and ax, 0x7FFF
        jz .time_est
        mov si, word_dynabook_anom
        call print_str
        jmp .time_done
.time_est:
        mov si, word_estimate
        call print_str
        jmp .time_done
.time_unk:
        mov si, word_unknown
        call print_str
.time_done:
        call print_crlf
        jmp .probe_vbepm

.apm_conn_err:
        mov si, msg_apm_connerr
        call print_str
        mov al, [apm_last_err]
        call print_hex8
        call print_crlf
        jmp .probe_vbepm

.apm_stat_err:
        mov [apm_last_err], ah
        call apm_disconnect_if_ours
        mov si, msg_apm_staterr
        call print_str
        mov al, [apm_last_err]
        call print_hex8
        call print_crlf
        jmp .probe_vbepm

.apm_absent:
        mov si, msg_apm_absent
        call print_str

        ; --- 2. VBE/PM 1.0 probe ---
.probe_vbepm:
        call probe_vbepm
        cmp byte [vbepm_ok], 0
        je .vbepm_absent

        mov si, msg_vbepm_ok
        call print_str
        mov al, [vbepm_ver]
        shr al, 4
        add al, '0'
        call print_char
        mov al, '.'
        call print_char
        mov al, [vbepm_ver]
        and al, 0x0F
        add al, '0'
        call print_char
        mov si, msg_caps
        call print_str
        mov al, [vbepm_caps]
        call print_hex8
        mov si, msg_states_open
        call print_str
        mov bl, [vbepm_caps]
        call print_vbepm_caps_list
        mov al, ')'
        call print_char
        call print_crlf
        jmp .probe_idle

.vbepm_absent:
        mov si, msg_vbepm_no
        call print_str
        mov ax, [vbepm_ax]
        call print_hex16
        mov al, ')'
        call print_char
        call print_crlf

        ; --- 3. CPU idle HLT TSR probe ---
.probe_idle:
        call query_idle_tsr
        mov si, msg_idle_hdr
        call print_str
        cmp byte [idle_tsr_state], 2
        je .idle_on
        cmp byte [idle_tsr_state], 1
        je .idle_off
        mov si, word_unloaded
        call print_str
        call print_crlf
        jmp .print_summary
.idle_on:
        mov si, word_on
        call print_str
        call print_idle_details
        call print_crlf
        jmp .print_summary
.idle_off:
        mov si, word_off
        call print_str
        call print_idle_details
        call print_crlf

        ; --- 4. Machine-readable summary line ---
.print_summary:
        mov si, msg_sum_head
        call print_str
        mov al, [apm_present]
        add al, '0'
        call print_char
        mov si, msg_sum_ver
        call print_str
        cmp byte [apm_present], 0
        je .sum_ver_none
        mov ax, [apm_ver]
        call print_bcd_hi
        mov al, '.'
        call print_char
        mov ax, [apm_ver]
        xchg ah, al
        call print_bcd_hi
        jmp .sum_ac
.sum_ver_none:
        mov si, word_none
        call print_str
.sum_ac:
        mov si, msg_sum_ac
        call print_str
        mov al, [ac_status]
        xor ah, ah
        call print_dec16
        mov si, msg_sum_batt
        call print_str
        mov al, [batt_status]
        xor ah, ah
        call print_dec16
        mov si, msg_sum_pct
        call print_str
        mov al, [batt_pct]
        xor ah, ah
        call print_dec16
        mov si, msg_sum_vbepm
        call print_str
        mov al, [vbepm_ok]
        add al, '0'
        call print_char
        mov si, msg_sum_caps
        call print_str
        mov al, [vbepm_caps]
        call print_hex8
        mov si, msg_sum_idle
        call print_str
        call print_idle_word
        call print_crlf

        mov ax, 0x4C00
        int 0x21

; =============================================================================
; VBE/PM 1.0 Display Control: ON, STANDBY, SUSPEND, OFF, BLANK
; =============================================================================
cmd_do_disp_on:
        mov byte [req_vbe_state], 0x00
        mov byte [req_vbe_cap], 0x00
        jmp disp_set_state

cmd_do_disp_standby:
        mov byte [req_vbe_state], 0x01
        mov byte [req_vbe_cap], 0x01
        jmp disp_set_state

cmd_do_disp_suspend:
        mov byte [req_vbe_state], 0x02
        mov byte [req_vbe_cap], 0x02
        jmp disp_set_state

cmd_do_disp_off:
        mov byte [req_vbe_state], 0x04
        mov byte [req_vbe_cap], 0x04
        jmp disp_set_state

disp_set_state:
        call probe_vbepm
        cmp byte [vbepm_ok], 0
        je .unsupported

        mov al, [req_vbe_cap]
        or al, al
        jz .do_call
        test [vbepm_caps], al
        jz .state_unsupported

.do_call:
        mov ax, 0x4F10
        mov bl, 0x01
        mov bh, [req_vbe_state]
        int 0x10
        cmp ax, 0x004F
        jne .call_failed

        mov si, msg_disp_ok
        call print_str
        mov al, [req_vbe_state]
        call print_hex8
        call print_crlf
        mov ax, 0x4C00
        int 0x21

.unsupported:
        mov si, msg_vbepm_no
        call print_str
        mov ax, [vbepm_ax]
        call print_hex16
        mov al, ')'
        call print_char
        call print_crlf
        mov si, msg_disp_unsup
        call print_str
        mov ax, 0x4C01
        int 0x21

.state_unsupported:
        mov si, msg_state_unsup
        call print_str
        mov al, [vbepm_caps]
        call print_hex8
        call print_crlf
        mov ax, 0x4C01
        int 0x21

.call_failed:
        push ax
        mov si, msg_disp_fail
        call print_str
        pop ax
        call print_hex16
        call print_crlf
        mov ax, 0x4C01
        int 0x21

cmd_do_blank:
        call probe_vbepm
        cmp byte [vbepm_ok], 0
        je disp_set_state.unsupported

        ; Select deepest supported blanking state: OFF(4) > SUSPEND(2) > STANDBY(1) > REDUCED(8)
        mov al, [vbepm_caps]
        test al, 0x04
        jnz .use_off
        test al, 0x02
        jnz .use_susp
        test al, 0x01
        jnz .use_stby
        test al, 0x08
        jnz .use_red
        jmp disp_set_state.state_unsupported
.use_off:
        mov byte [req_vbe_state], 0x04
        jmp .do_blank
.use_susp:
        mov byte [req_vbe_state], 0x02
        jmp .do_blank
.use_stby:
        mov byte [req_vbe_state], 0x01
        jmp .do_blank
.use_red:
        mov byte [req_vbe_state], 0x08
.do_blank:
        mov si, msg_blanking
        call print_str
        mov al, [req_vbe_state]
        call print_hex8
        mov si, msg_blank_wait
        call print_str

        mov ax, 0x4F10
        mov bl, 0x01
        mov bh, [req_vbe_state]
        int 0x10
        cmp ax, 0x004F
        jne disp_set_state.call_failed

        ; Wait for any keystroke, then restore display ON (BH=00h)
        xor ah, ah
        int 0x16

        mov ax, 0x4F10
        mov bx, 0x0001          ; BL=01h (Set State), BH=00h (ON)
        int 0x10

        mov si, msg_blank_done
        call print_str
        mov ax, 0x4C00
        int 0x21

; =============================================================================
; APM 1.2 System Sleep / Power-Off: SLEEP [STANDBY|SUSPEND], POWEROFF
; =============================================================================
cmd_do_sleep:
        mov word [req_apm_state], 0x0002        ; Default: Suspend
        cmp byte [arg2_buf], 0
        je apm_enter_state
        mov si, arg2_buf
        mov di, cmd_standby
        call str_eq
        jc .sleep_stby
        mov si, arg2_buf
        mov di, cmd_suspend
        call str_eq
        jc apm_enter_state
        mov si, msg_bad_sleep
        call print_str
        mov ax, 0x4C01
        int 0x21
.sleep_stby:
        mov word [req_apm_state], 0x0001
        jmp apm_enter_state

cmd_do_poweroff:
        mov word [req_apm_state], 0x0003        ; Off
        jmp apm_enter_state

apm_enter_state:
        call probe_apm_check
        cmp byte [apm_present], 0
        je .no_apm

        call apm_connect_rm
        jc .conn_err

        ; Negotiate APM 1.2 driver version (required by APM 1.2 BIOSes for power-off)
        mov ax, 0x530E
        xor bx, bx
        mov cx, 0x0102
        int 0x15
        jnc .ver_ok
        mov ax, 0x530E
        xor bx, bx
        mov cx, [apm_ver]
        int 0x15
.ver_ok:
        ; Enable power management for all devices (5308h BX=0001h CX=0001h)
        mov ax, 0x5308
        mov bx, 0x0001
        mov cx, 0x0001
        int 0x15

        ; Flush DOS buffers before state transition
        mov ah, 0x0D
        int 0x21

        mov si, msg_apm_req
        call print_str
        mov ax, [req_apm_state]
        call print_hex16
        call print_crlf

        ; INT 15h AX=5307h BX=0001h (all devices) CX=state (1=standby, 2=suspend, 3=off)
        mov ax, 0x5307
        mov bx, 0x0001
        mov cx, [req_apm_state]
        int 0x15
        jc .rejected

        call apm_disconnect_if_ours
        cmp word [req_apm_state], 0x0003
        je .poweroff_returned
        mov si, msg_apm_resumed
        call print_str
        mov ax, 0x4C00
        int 0x21

.poweroff_returned:
        mov si, msg_poff_fallback
        call print_str
        mov ax, 0x4C01
        int 0x21

.rejected:
        mov [apm_last_err], ah
        call apm_disconnect_if_ours
        mov si, msg_apm_rej
        call print_str
        mov al, [apm_last_err]
        call print_hex8
        call print_crlf
        cmp word [req_apm_state], 0x0003
        jne .rej_exit
        mov si, msg_poff_fallback
        call print_str
.rej_exit:
        mov ax, 0x4C01
        int 0x21

.no_apm:
        mov si, msg_apm_absent
        call print_str
        cmp word [req_apm_state], 0x0003
        jne .no_apm_exit
        mov si, msg_poff_fallback
        call print_str
.no_apm_exit:
        mov ax, 0x4C01
        int 0x21

.conn_err:
        mov si, msg_apm_connerr
        call print_str
        mov al, [apm_last_err]
        call print_hex8
        call print_crlf
        mov ax, 0x4C01
        int 0x21

; =============================================================================
; CPU Idle HLT TSR Control: IDLE [ON|OFF|STATUS]
; =============================================================================
cmd_do_idle:
        cmp byte [arg2_buf], 0
        je .idle_status
        mov si, arg2_buf
        mov di, cmd_status
        call str_eq
        jc .idle_status
        mov si, arg2_buf
        mov di, cmd_on
        call str_eq
        jc .idle_enable
        mov si, arg2_buf
        mov di, cmd_off
        call str_eq
        jc .idle_disable

        mov si, msg_bad_idle
        call print_str
        mov ax, 0x4C01
        int 0x21

.idle_status:
        call query_idle_tsr
        mov si, msg_idle_hdr
        call print_str
        call print_idle_word
        cmp byte [idle_tsr_state], 0
        je .idle_stat_nl
        call print_idle_details
.idle_stat_nl:
        call print_crlf
        mov si, msg_idle_sum
        call print_str
        call print_idle_word
        call print_crlf
        mov ax, 0x4C00
        int 0x21

.idle_enable:
        mov ax, PWRCTL_MUX_AX
        mov bx, PWRCTL_MUX_BX
        mov cl, 1               ; enable if already resident
        int 0x2F
        cmp ax, PWRCTL_MUX_ACK
        jne .install_tsr

        mov si, msg_idle_enabled
        call print_str
        mov ax, 0x4C00
        int 0x21

.install_tsr:
        mov byte [idle_enabled], 1
        mov word [idle_count], 0

        ; Save original INT 28h
        mov ax, 0x3528
        int 0x21
        mov [old_int28], bx
        mov [old_int28 + 2], es

        ; Save original INT 2Fh
        mov ax, 0x352F
        int 0x21
        mov [old_int2f], bx
        mov [old_int2f + 2], es

        push cs
        pop es

        ; Hook INT 28h and INT 2Fh
        mov ax, 0x2528
        mov dx, isr_int28
        int 0x21

        mov ax, 0x252F
        mov dx, isr_int2f
        int 0x21

        ; Free our environment block to minimize conventional memory footprint
        mov ax, [0x2C]
        test ax, ax
        jz .skip_env_free
        mov es, ax
        mov ah, 0x49
        int 0x21
        push cs
        pop es
.skip_env_free:
        mov si, msg_idle_installed
        call print_str

        mov dx, (resident_end - $$ + 0x100 + 15) / 16
        mov ax, 0x3100
        int 0x21

.idle_disable:
        mov ax, PWRCTL_MUX_AX
        mov bx, PWRCTL_MUX_BX
        mov cl, 2               ; disable if resident
        int 0x2F
        cmp ax, PWRCTL_MUX_ACK
        jne .not_loaded

        mov si, msg_idle_disabled
        call print_str
        mov ax, 0x4C00
        int 0x21

.not_loaded:
        mov si, msg_idle_not_loaded
        call print_str
        mov ax, 0x4C00
        int 0x21

; =============================================================================
; Hardware / BIOS Probe Helpers
; =============================================================================
probe_apm_check:
        mov byte [apm_present], 0
        mov ax, 0x5300
        xor bx, bx
        int 0x15
        jc .done
        cmp bx, 0x504D          ; 'PM'
        jne .done
        mov byte [apm_present], 1
        mov [apm_ver], ax
        mov [apm_flags], cx
.done:
        ret

; Connect real-mode APM interface; tolerates AH=02h (already connected).
; Sets CF=1 on fatal error and stores AH in [apm_last_err].
apm_connect_rm:
        mov byte [apm_we_connected], 0
        mov ax, 0x5301
        xor bx, bx
        int 0x15
        jnc .ok
        cmp ah, 0x02            ; Real-mode interface already connected
        je .already
        mov [apm_last_err], ah
        stc
        ret
.ok:
        mov byte [apm_we_connected], 1
.already:
        clc
        ret

apm_disconnect_if_ours:
        cmp byte [apm_we_connected], 0
        je .skip
        mov ax, 0x5304
        xor bx, bx
        int 0x15
        mov byte [apm_we_connected], 0
.skip:
        ret

probe_vbepm:
        mov byte [vbepm_ok], 0
        mov byte [vbepm_ver], 0
        mov byte [vbepm_caps], 0
        push es
        xor di, di
        mov es, di
        xor cx, cx
        mov ax, 0x4F10
        xor bx, bx
        int 0x10
        pop es
        mov [vbepm_ax], ax
        cmp ax, 0x004F
        jne .done
        mov byte [vbepm_ok], 1
        mov [vbepm_ver], bl
        mov [vbepm_caps], bh
.done:
        ret

query_idle_tsr:
        mov byte [idle_tsr_state], 0
        mov ax, PWRCTL_MUX_AX
        mov bx, PWRCTL_MUX_BX
        xor cl, cl
        int 0x2F
        cmp ax, PWRCTL_MUX_ACK
        jne .done
        mov [idle_tsr_seg], cx
        mov [idle_tsr_hlts], dx
        or bl, bl
        jz .off
        mov byte [idle_tsr_state], 2
        ret
.off:
        mov byte [idle_tsr_state], 1
.done:
        ret

; =============================================================================
; CLI Parsing & Printing Helpers
; =============================================================================
skip_ws:
        test cx, cx
        jz .done
.loop:
        mov al, [si]
        cmp al, ' '
        je .next
        cmp al, 0x09
        jne .done
.next:
        inc si
        dec cx
        jnz .loop
.done:
        ret

; Read whitespace/CR-delimited token (up to 15 chars), uppercased, NUL-terminated.
read_upper_token:
        mov bx, 15
.loop:
        test cx, cx
        jz .end
        mov al, [si]
        cmp al, ' '
        je .end
        cmp al, 0x09
        je .end
        cmp al, 0x0D
        je .end
        cmp al, 0x00
        je .end
        inc si
        dec cx
        test bx, bx
        jz .loop
        cmp al, 'a'
        jb .store
        cmp al, 'z'
        ja .store
        sub al, 32
.store:
        mov [di], al
        inc di
        dec bx
        jmp .loop
.end:
        mov byte [di], 0
        ret

; Compare NUL-terminated strings at DS:SI and DS:DI. CF=1 if equal, CF=0 otherwise.
str_eq:
.loop:
        mov al, [si]
        cmp al, [di]
        jne .ne
        or al, al
        jz .eq
        inc si
        inc di
        jmp .loop
.eq:
        stc
        ret
.ne:
        clc
        ret

print_vbepm_caps_list:
        xor ch, ch              ; printed count
        test bl, 0x01
        jz .c2
        mov si, word_c_stby
        call print_cap_item
.c2:
        test bl, 0x02
        jz .c4
        mov si, word_c_susp
        call print_cap_item
.c4:
        test bl, 0x04
        jz .c8
        mov si, word_c_off
        call print_cap_item
.c8:
        test bl, 0x08
        jz .cdone
        mov si, word_c_red
        call print_cap_item
.cdone:
        or ch, ch
        jnz .ret
        mov si, word_none
        call print_str
.ret:
        ret

print_cap_item:
        or ch, ch
        jz .first
        mov al, ','
        call print_char
.first:
        call print_str
        inc ch
        ret

print_idle_word:
        cmp byte [idle_tsr_state], 2
        je .w_on
        cmp byte [idle_tsr_state], 1
        je .w_off
        mov si, word_unloaded
        jmp print_str
.w_on:
        mov si, word_on
        jmp print_str
.w_off:
        mov si, word_off
        jmp print_str

print_idle_details:
        mov si, msg_idle_seg
        call print_str
        mov ax, [idle_tsr_seg]
        call print_hex16
        mov si, msg_idle_hlts
        call print_str
        mov ax, [idle_tsr_hlts]
        call print_dec16
        mov al, ')'
        jmp print_char

print_bcd_hi:
        push ax
        mov al, ah
        shr al, 4
        or al, al
        jz .low
        add al, '0'
        call print_char
.low:
        pop ax
        mov al, ah
        and al, 0x0F
        add al, '0'
        jmp print_char

print_hex16:
        push ax
        mov al, ah
        call print_hex8
        pop ax
print_hex8:
        push ax
        shr al, 4
        call print_hex_nib
        pop ax
        and al, 0x0F
print_hex_nib:
        cmp al, 9
        jbe .digit
        add al, 'A' - 10
        jmp print_char
.digit:
        add al, '0'
        jmp print_char

print_dec16:
        push bx
        push cx
        push dx
        mov bx, 10
        xor cx, cx
.div:
        xor dx, dx
        div bx
        push dx
        inc cx
        or ax, ax
        jnz .div
.out:
        pop ax
        add al, '0'
        call print_char
        loop .out
        pop dx
        pop cx
        pop bx
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

print_str:
        push ax
        push dx
        mov ah, 0x09
        mov dx, si
        int 0x21
        pop dx
        pop ax
        ret

print_crlf:
        mov si, msg_crlf
        jmp print_str

; =============================================================================
; M7101 PMU Commands: BATTERY, THROTTLE, BRIGHT (via FERRIC.JLM 0x0039..0x003B)
; =============================================================================
find_ferric_jlm:
        push es
        xor di, di
        mov es, di
        mov ax, 0x1684
        mov bx, 0xFD01
        int 0x2F
        mov [jlm_off], di
        mov ax, es
        mov [jlm_seg], ax
        pop es
        or ax, di
        jnz .ok
        mov si, msg_no_jlm
        call print_str
        stc
        ret
.ok:
        clc
        ret

print_jlm_buf:
        mov cx, ax
        test cx, cx
        jz .pjb_done
        mov si, jlm_io_buf
.pjb_ch:
        lodsb
        call print_char
        loop .pjb_ch
.pjb_done:
        ret

cmd_do_battery:
        call find_ferric_jlm
        jc .err
        push cs
        pop es
        mov di, jlm_io_buf
        mov cx, 1024
        mov ax, 0x0039
        call far [jlm_off]
        push cs
        pop es
        call print_jlm_buf
        mov ax, 0x4C00
        int 0x21
.err:
        mov ax, 0x4C01
        int 0x21

cmd_do_throttle:
        call find_ferric_jlm
        jc .err
        ; No arg -> show M7101 info
        cmp byte [arg2_buf], 0
        je cmd_do_battery
        ; "OFF" -> level 0
        mov si, arg2_buf
        mov di, cmd_off
        call str_eq
        jc .set_zero
        ; Single digit '0'..'8' ('0' or '8' = OFF / 100% speed)
        mov al, [arg2_buf]
        cmp al, '0'
        jb .bad_arg
        cmp al, '8'
        ja .bad_arg
        cmp al, '8'
        je .set_zero
        sub al, '0'
        movzx bx, al
        jmp .do_set
.set_zero:
        xor bx, bx
.do_set:
        push cs
        pop es
        mov di, jlm_io_buf
        mov cx, 1024
        mov ax, 0x003A
        call far [jlm_off]
        push cs
        pop es
        call print_jlm_buf
        mov ax, 0x4C00
        int 0x21
.bad_arg:
        mov si, msg_bad_thro
        call print_str
        mov ax, 0x4C01
        int 0x21
.err:
        mov ax, 0x4C01
        int 0x21

cmd_do_bright:
        call find_ferric_jlm
        jc .err
        cmp byte [arg2_buf], 0
        je cmd_do_battery
        mov al, [arg2_buf]
        cmp al, '0'
        jb .bad_arg
        cmp al, '8'
        ja .bad_arg
        sub al, '0'
        movzx bx, al
        push cs
        pop es
        mov di, jlm_io_buf
        mov cx, 1024
        mov ax, 0x003B
        call far [jlm_off]
        push cs
        pop es
        call print_jlm_buf
        mov ax, 0x4C00
        int 0x21
.bad_arg:
        mov si, msg_bad_bright
        call print_str
        mov ax, 0x4C01
        int 0x21
.err:
        mov ax, 0x4C01
        int 0x21

; =============================================================================
; Data Section
; =============================================================================
apm_present:      db 0
apm_we_connected: db 0
apm_stat_ok:      db 0
apm_last_err:     db 0
apm_ver:          dw 0
apm_flags:        dw 0
ac_status:        db 0xFF
batt_status:      db 0xFF
batt_pct:         db 0xFF
batt_time:        dw 0xFFFF

vbepm_ok:         db 0
vbepm_ver:        db 0
vbepm_caps:       db 0
vbepm_ax:         dw 0
req_vbe_state:    db 0
req_vbe_cap:      db 0
req_apm_state:    dw 0

idle_tsr_state:   db 0          ; 0=UNLOADED, 1=OFF, 2=ON
idle_tsr_seg:     dw 0
idle_tsr_hlts:    dw 0

arg1_buf:         times 16 db 0
arg2_buf:         times 16 db 0

cmd_status:       db 'STATUS', 0
cmd_blank:        db 'BLANK', 0
cmd_on:           db 'ON', 0
cmd_standby:      db 'STANDBY', 0
cmd_suspend:      db 'SUSPEND', 0
cmd_off:          db 'OFF', 0
cmd_sleep:        db 'SLEEP', 0
cmd_poweroff:     db 'POWEROFF', 0
cmd_idle:         db 'IDLE', 0
cmd_battery:      db 'BATTERY', 0
cmd_batt:         db 'BATT', 0
cmd_throttle:     db 'THROTTLE', 0
cmd_bright:       db 'BRIGHT', 0
cmd_help:         db 'HELP', 0
cmd_qmark:        db '?', 0
cmd_slashq:       db '/?', 0

msg_title:        db 'Ferric DOS Power Control (PWRCTL)', 0x0D, 0x0A, '$'
msg_usage:        db 'Usage: PWRCTL <command>', 0x0D, 0x0A
                  db '  STATUS                  APM 1.2 + VBE/PM 1.0 + CPU idle status', 0x0D, 0x0A
                  db '  BLANK                   VBE/PM display blank (any key wakes)', 0x0D, 0x0A
                  db '  ON|STANDBY|SUSPEND|OFF  VBE/PM 1.0 display power state', 0x0D, 0x0A
                  db '  SLEEP [STANDBY|SUSPEND] APM 1.2 system standby/suspend', 0x0D, 0x0A
                  db '  POWEROFF                APM 1.2 system power-off', 0x0D, 0x0A
                  db '  IDLE [ON|OFF|STATUS]    CPU idle HLT TSR (INT 28h + INT 2Fh 1680h)', 0x0D, 0x0A
                  db '  BATTERY                 ALi M7101 Smart Battery + throttle + backlight', 0x0D, 0x0A
                  db '  THROTTLE [OFF|1..8]     ALi M7101 CPU clock throttle (1..7=duty/8, 8/OFF=100%)', 0x0D, 0x0A
                  db '  BRIGHT [0..8]           ALi M7101 LCD CCFT backlight (0=off, 8=max)', 0x0D, 0x0A, '$'
msg_no_jlm:       db 'PWRCTL: FERRIC.JLM (0xFD01) not loaded.', 0x0D, 0x0A, '$'
msg_bad_thro:     db 'Usage: PWRCTL THROTTLE [OFF|0..8]', 0x0D, 0x0A, '$'
msg_bad_bright:   db 'Usage: PWRCTL BRIGHT [0..8]', 0x0D, 0x0A, '$'
jlm_off:          dw 0
jlm_seg:          dw 0
msg_bad_cmd:      db 'Unknown command.', 0x0D, 0x0A, '$'
msg_bad_sleep:    db 'Usage: PWRCTL SLEEP [STANDBY|SUSPEND]', 0x0D, 0x0A, '$'
msg_bad_idle:     db 'Usage: PWRCTL IDLE [ON|OFF|STATUS]', 0x0D, 0x0A, '$'

msg_apm_ver:      db 'APM version: $'
msg_flags:        db ' (flags=0x$'
msg_apm_absent:   db 'APM: not present', 0x0D, 0x0A, '$'
msg_apm_connerr:  db 'APM connect failed, err=0x$'
msg_apm_staterr:  db 'APM status failed, err=0x$'
msg_ac:           db 'AC line: $'
msg_batt:         db 'Battery: $'
msg_pct:          db 'Charge: $'
msg_time:         db 'Remaining time: $'

msg_vbepm_ok:     db 'VBE/PM: v$'
msg_caps:         db ' caps=0x$'
msg_states_open:  db ' ($'
msg_vbepm_no:     db 'VBE/PM: not supported (AX=0x$'

msg_disp_ok:      db 'VBE/PM display state set: 0x$'
msg_disp_unsup:   db 'PWRCTL: display=UNSUPPORTED', 0x0D, 0x0A, '$'
msg_state_unsup:  db 'Requested VBE/PM state not supported (caps=0x$'
msg_disp_fail:    db 'VBE/PM set state failed (AX=0x$'
msg_blanking:     db 'Blanking display (VBE/PM state 0x$'
msg_blank_wait:   db '), press any key to wake...', 0x0D, 0x0A, '$'
msg_blank_done:   db 'Display restored (ON).', 0x0D, 0x0A, '$'

msg_apm_req:      db 'Requesting APM system state 0x$'
msg_apm_resumed:  db 'Resumed from APM sleep.', 0x0D, 0x0A, '$'
msg_apm_rej:      db 'APM rejected request, err=0x$'
msg_poff_fallback: db 'Safe to power off manually.', 0x0D, 0x0A, '$'

msg_idle_hdr:     db 'CPU idle HLT: $'
msg_idle_seg:     db ' (seg=0x$'
msg_idle_hlts:    db ', hlts=$'
msg_idle_sum:     db 'PWRCTL: idle=$'
msg_idle_installed: db 'CPU idle HLT TSR installed (INT 28h + INT 2Fh AX=1680h).', 0x0D, 0x0A
                    db 'PWRCTL: idle=ON', 0x0D, 0x0A, '$'
msg_idle_enabled:   db 'CPU idle HLT enabled.', 0x0D, 0x0A
                    db 'PWRCTL: idle=ON', 0x0D, 0x0A, '$'
msg_idle_disabled:  db 'CPU idle HLT disabled.', 0x0D, 0x0A
                    db 'PWRCTL: idle=OFF', 0x0D, 0x0A, '$'
msg_idle_not_loaded: db 'CPU idle HLT TSR not loaded.', 0x0D, 0x0A
                     db 'PWRCTL: idle=UNLOADED', 0x0D, 0x0A, '$'

msg_sum_head:     db 'PWRCTL: apm=$'
msg_sum_ver:      db ' ver=$'
msg_sum_ac:       db ' ac=$'
msg_sum_batt:     db ' batt=$'
msg_sum_pct:      db ' pct=$'
msg_sum_vbepm:    db ' vbepm=$'
msg_sum_caps:     db ' caps=0x$'
msg_sum_idle:     db ' idle=$'
msg_crlf:         db 0x0D, 0x0A, '$'

word_online:      db 'online$'
word_offline:     db 'offline$'
word_backup:      db 'on backup power$'
word_high:        db 'high$'
word_low:         db 'low$'
word_critical:    db 'critical$'
word_charging:    db 'charging$'
word_absent:      db 'not present$'
word_unknown:     db 'unknown$'
word_minutes:     db ' minutes$'
word_seconds:     db ' seconds$'
word_estimate:    db ' (BIOS estimate)$'
word_dynabook_anom: db ' (BIOS estimate; known DynaBook AC-on-line anomaly)$'
word_none:        db 'NONE$'
word_on:          db 'ON$'
word_off:         db 'OFF$'
word_unloaded:    db 'UNLOADED$'
word_c_stby:      db 'STANDBY$'
word_c_susp:      db 'SUSPEND$'
word_c_off:       db 'OFF$'
word_c_red:       db 'REDUCED_ON$'
jlm_io_buf:       times 1024 db 0
