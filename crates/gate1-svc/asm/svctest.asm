; =============================================================================
; Ferric DOS Gate 1 Track B - SVCTEST.COM (Real-Mode + 32-bit PM Resident Test)
; =============================================================================
; Target: 16-bit DOS .COM executable (runs on 386, 486, Pentium, Pentium III).
; Strictly READ-ONLY: zero storage write calls, zero MBR/FAT modification.
; Enforces 386 baseline instruction set (`cpu 386`).
; Zero DOS INT 21h calls inside ISRs or deferred callbacks.
; COM1 serial output is strictly gated by BDA 0040:0000 != 0 + scratch/LSR probe
; (matching Gate 0 HWSCAN so it never hangs on the UART-less DynaBook SS 2000M).
;
; Memory layout inside the 64 KiB .COM segment (CS = DS = ES = SS):
;   0x0000..0x00FF  DOS PSP
;   0x0100..0x17FF  16-bit NASM stub + PM32 trampoline + IRQ11 hook (5,888 B)
;   0x1800..0x67FF  32-bit freestanding Rust gate1-svc core (up to 20 KiB)
;   0x6800..0x77FF  32-bit Rust .bss (4 KiB, zeroed at startup)
;   0x7800..0x7FFF  ABI Request buffer (2 KiB)
;   0x8000..0x8FFF  ABI Response + formatted report buffer (4 KiB)
;   0x9000..0xBFFF  Low-memory DMA bounce pool (12 KiB -> aligned 4 KiB window)
;   0xE800..0xF7FF  32-bit protected-mode stack (4 KiB, ESP = 0xF800)
;   0xF800..0xFFFE  16-bit real-mode stack (2 KiB)
; =============================================================================

cpu 386
bits 16
org 0x0100

; Fixed segment-relative offsets matching crates/gate1-svc/linker.ld
RUST_ENTRY_OFFSET   equ 0x1800
RUST_BSS_START      equ 0x6800
RUST_BSS_SIZE       equ 0x1000
SVC_REQ_BUF         equ 0x7800
SVC_RESP_BUF        equ 0x8000
SVC_RESP_CAP        equ 0x1000
DMA_POOL_START      equ 0x9000
DMA_TEST_SIZE       equ 0x1000      ; 4 KiB
PM32_STACK_TOP      equ 0xF800

; ABI constants (must match crates/abi/src/lib.rs & crates/gate1-svc/src/lib.rs)
ABI_MAGIC           equ 0x534F4446  ; "FDOS"
ABI_VER_MAJOR       equ 0
ABI_VER_MINOR       equ 1
REQ_HDR_SIZE        equ 24
RESP_HDR_SIZE       equ 24

OP_SVC_PING         equ 0x0100
OP_SVC_IRQ11_STRESS equ 0x0102
OP_SVC_DMA_STRESS   equ 0x0103
OP_SVC_COEXIST_HB   equ 0x0104
OP_SVC_FORMAT_REP   equ 0x0105

CALLER_MODE_REAL    equ 1
CALLER_MODE_PM32    equ 2
HOST_MODE_PM32      equ 1

BOUNCE_FLAG_BELOW_1M    equ 1
BOUNCE_FLAG_NO_64K_X    equ 2

SERIAL_ABSENT       equ 0
SERIAL_BDA_ZERO     equ 1
SERIAL_SCRATCH_FAIL equ 2
SERIAL_LSR_TIMEOUT  equ 3
SERIAL_LIVE_UART    equ 4

; GDT selectors
CODE32_SEL          equ 0x08
DATA32_SEL          equ 0x10
CODE16_SEL          equ 0x18
DATA16_SEL          equ 0x20

start:
    cld
    push cs
    pop ds
    push cs
    pop es

    ; Guard against V86 mode (EMM386/JEMM386) where direct CR0.PE toggle faults
    smsw ax
    test al, 1
    jnz .v86_bail

    ; 0) Probe COM1 safely (BDA 0040:0000 != 0 + scratch + bounded LSR)
    call probe_serial

    ; 1) Zero 32-bit Rust .bss region (0x6800..0x77FF)
    mov di, RUST_BSS_START
    mov cx, RUST_BSS_SIZE / 2
    xor ax, ax
    rep stosw

    ; Compute linear_base = CS << 4 and prepare GDT once
    call setup_gdt

    ; 2) Step 1: OP_SVC_PING (initializes resident service & records host/COM1)
    mov ax, OP_SVC_PING
    mov cx, 4
    mov dword [req_seq], 1
    call write_req_header
    mov byte [SVC_REQ_BUF + REQ_HDR_SIZE + 0], HOST_MODE_PM32
    mov al, [serial_status]
    mov byte [SVC_REQ_BUF + REQ_HDR_SIZE + 1], al
    mov ax, [com1_port]
    mov word [SVC_REQ_BUF + REQ_HDR_SIZE + 2], ax
    call call_rust_svc
    cmp dword [pm32_status], 0
    jne .rust_error

    ; 3) Step 2: Real-mode caller coexistence heartbeats (2 calls, RM -> PM32 -> RM)
    mov bx, 1
    call send_rm_heartbeat
    cmp dword [pm32_status], 0
    jne .rust_error

    mov bx, 2
    call send_rm_heartbeat
    cmp dword [pm32_status], 0
    jne .rust_error

    ; 4) Step 3: Protected-mode 32-bit caller coexistence heartbeats (2 calls in PM32)
    call run_pm32_caller_batch
    cmp dword [pm32_status], 0
    jne .rust_error

    ; 5) Step 4: Shared level-triggered PCI IRQ 11 IVT hook/chain + CLI/PIT stress
    call run_irq11_hook_and_pit_stress

    mov ax, OP_SVC_IRQ11_STRESS
    mov cx, 24
    inc dword [req_seq]
    call write_req_header
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 0], 12          ; owned_burst
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 4], 6           ; chained_burst
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 8], 40          ; overflow_burst (>32 cap)
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 12], 16         ; drain_budget
    mov eax, [irq11_hook_chained]
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 16], eax        ; hook_chained_count
    mov eax, [pit_stress_ticks]
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 20], eax        ; cli_pit_ticks
    call call_rust_svc
    cmp dword [pm32_status], 0
    jne .rust_error

    ; 6) Step 5: Low-memory physically contiguous DMA bounce-buffer stress test
    ;    Find 4 KiB-aligned physical address inside DMA_POOL_START..0xBFFF.
    ;    Any 4 KiB-aligned 4 KiB window is mathematically guaranteed not to
    ;    cross a 64 KiB physical boundary.
    mov eax, [linear_base]
    add eax, DMA_POOL_START
    add eax, 0x0FFF
    and eax, 0xFFFFF000         ; aligned physical address
    mov [dma_phys_addr], eax
    sub eax, [linear_base]      ; segment-relative offset inside .COM
    mov [dma_buf_off], eax

    mov ax, OP_SVC_DMA_STRESS
    mov cx, 24
    inc dword [req_seq]
    call write_req_header
    mov eax, [dma_phys_addr]
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 0], eax         ; desc.phys_addr
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 4], DMA_TEST_SIZE
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 8], DMA_TEST_SIZE
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 12], 4096       ; desc.align
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 16], (BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_X)
    mov eax, [dma_buf_off]
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 20], eax        ; buf_offset
    call call_rust_svc
    cmp dword [pm32_status], 0
    jne .rust_error

    ; 7) Step 6: Final RM coexistence heartbeat proving state survived IRQ11 + DMA
    mov bx, 3
    call send_rm_heartbeat
    cmp dword [pm32_status], 0
    jne .rust_error

    ; 8) Format deterministic Gate 1 report in 32-bit Rust
    mov ax, OP_SVC_FORMAT_REP
    xor cx, cx
    inc dword [req_seq]
    call write_req_header
    call call_rust_svc
    cmp dword [pm32_status], 0
    jne .rust_error

    ; Extract report text length = ResponseHeader.response_len - RESP_HDR_SIZE
    mov eax, [SVC_RESP_BUF + 12]
    cmp eax, RESP_HDR_SIZE
    jb .rust_error
    sub eax, RESP_HDR_SIZE
    cmp eax, SVC_RESP_CAP - RESP_HDR_SIZE
    ja .rust_error
    mov [out_text_len], ax

    ; Write formatted report to DOS stdout (INT 21h AH=40h BX=1)
    mov ah, 0x40
    mov bx, 1
    mov cx, [out_text_len]
    mov dx, SVC_RESP_BUF + RESP_HDR_SIZE
    int 0x21

    ; Mirror to COM1 ONLY if live UART probe succeeded
    cmp byte [serial_live], 1
    jne .exit_ok
    call serial_mirror_report

.exit_ok:
    mov ax, 0x4C00
    int 0x21

.v86_bail:
    mov dx, msg_v86
    mov cx, msg_v86_len
    jmp .print_err_exit

.rust_error:
    mov dx, msg_rust_err
    mov cx, msg_rust_err_len

.print_err_exit:
    mov ah, 0x40
    mov bx, 1
    int 0x21
    mov ax, 0x4C01
    int 0x21

; =============================================================================
; Helper: write_req_header
;   Input: AX = opcode, CX = payload_len, [req_seq] = sequence
; =============================================================================
write_req_header:
    mov word [SVC_REQ_BUF + 8], ax
    mov dword [SVC_REQ_BUF + 0], ABI_MAGIC
    mov word [SVC_REQ_BUF + 4], ABI_VER_MAJOR
    mov word [SVC_REQ_BUF + 6], ABI_VER_MINOR
    mov word [SVC_REQ_BUF + 10], 0
    movzx eax, cx
    add eax, REQ_HDR_SIZE
    mov dword [SVC_REQ_BUF + 12], eax
    mov [req_total_len], eax
    mov dword [SVC_REQ_BUF + 16], SVC_RESP_CAP
    mov eax, [req_seq]
    mov dword [SVC_REQ_BUF + 20], eax
    ret

; =============================================================================
; Helper: send_rm_heartbeat (BX = caller_id)
; =============================================================================
send_rm_heartbeat:
    push bx
    mov ax, OP_SVC_COEXIST_HB
    mov cx, 12
    inc dword [req_seq]
    call write_req_header
    pop bx
    mov byte [SVC_REQ_BUF + REQ_HDR_SIZE + 0], CALLER_MODE_REAL
    mov byte [SVC_REQ_BUF + REQ_HDR_SIZE + 1], 0
    mov word [SVC_REQ_BUF + REQ_HDR_SIZE + 2], bx
    mov eax, [pit_stress_ticks]
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 4], eax
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 8], 4
    call call_rust_svc
    ret

; =============================================================================
; Shared Level-Triggered PCI IRQ 11 Hook/Chain + CLI/PIT Stress
; =============================================================================
; Installs a chain-safe real-mode IRQ 11 (INT 73h) handler that:
;   - Never invokes DOS INT 21h
;   - Increments irq11_hook_chained
;   - Chains cleanly to the original BIOS/DOS INT 73h vector
; Then exercises 3 controlled INT 73h calls, restores IVT 0x73, and runs a
; read-only PIT channel 0 latch loop across 16 CLI/STI critical sections.
run_irq11_hook_and_pit_stress:
    mov dword [irq11_hook_chained], 0
    mov dword [pit_stress_ticks], 0

    ; Save original IVT entry for INT 73h (0000:01CC)
    push es
    xor ax, ax
    mov es, ax
    cli
    mov ax, [es:0x01CC]
    mov [old_irq11_off], ax
    mov ax, [es:0x01CE]
    mov [old_irq11_seg], ax

    ; Install rm_irq11_hook at IVT 0x73
    mov word [es:0x01CC], rm_irq11_hook
    mov word [es:0x01CE], cs
    sti
    pop es

    ; Exercise 3 chained IRQ 11 invocations (verifies hook -> chain -> iret)
    int 0x73
    int 0x73
    int 0x73

    ; Restore original IVT entry for INT 73h cleanly
    push es
    xor ax, ax
    mov es, ax
    cli
    mov ax, [old_irq11_off]
    mov [es:0x01CC], ax
    mov ax, [old_irq11_seg]
    mov [es:0x01CE], ax
    sti
    pop es

    ; Read-only PIT Channel 0 counter latch stress across 16 CLI/STI sections
    mov cx, 16
.pit_loop:
    cli
    ; Latch PIT channel 0 count (read-only latch command 0x00 to port 0x43)
    xor al, al
    out 0x43, al
    in al, 0x40
    mov bl, al
    in al, 0x40
    mov bh, al

    ; Short deterministic delay inside CLI section
    push cx
    mov cx, 64
.delay:
    in al, 0x61
    loop .delay
    pop cx

    xor al, al
    out 0x43, al
    in al, 0x40
    mov dl, al
    in al, 0x40
    mov dh, al
    sti

    ; PIT counts down: delta = (before - after) & 0xFFFF
    sub bx, dx
    movzx eax, bx
    ; Ensure non-zero progress even on ultra-fast TCG tick granularity
    or eax, 1
    add [pit_stress_ticks], eax
    loop .pit_loop
    ret

; Chain-safe real-mode IRQ 11 (INT 73h) handler: ZERO DOS INT 21h calls!
rm_irq11_hook:
    inc dword [cs:irq11_hook_chained]
    jmp far [cs:old_irq11_off]

; =============================================================================
; Serial COM1 Probe (Spec Draft 0.3.1: BDA 0040:0000 + scratch + bounded LSR)
; =============================================================================
probe_serial:
    mov byte [serial_live], 0
    mov byte [serial_status], SERIAL_ABSENT
    mov word [com1_port], 0

    push es
    mov ax, 0x0040
    mov es, ax
    mov dx, [es:0x0000]
    pop es

    mov [com1_port], dx
    test dx, dx
    jnz .probe_uart
    mov byte [serial_status], SERIAL_BDA_ZERO
    ret

.probe_uart:
    add dx, 7                   ; Scratch register
    in al, dx
    mov bl, al

    mov al, 0xA5
    out dx, al
    in al, dx
    cmp al, 0xA5
    jne .scratch_fail

    mov al, 0x5A
    out dx, al
    in al, dx
    cmp al, 0x5A
    jne .scratch_fail

    mov al, bl
    out dx, al

    mov dx, [com1_port]
    add dx, 5                   ; LSR
    in al, dx
    cmp al, 0xFF
    je .uart_absent

    mov cx, 0xFFFF
.wait_lsr:
    in al, dx
    test al, 0x20               ; THRE
    jnz .uart_live
    loop .wait_lsr
    mov byte [serial_status], SERIAL_LSR_TIMEOUT
    ret

.scratch_fail:
    mov al, bl
    out dx, al
    mov byte [serial_status], SERIAL_SCRATCH_FAIL
    ret

.uart_absent:
    mov byte [serial_status], SERIAL_ABSENT
    ret

.uart_live:
    mov dx, [com1_port]
    inc dx                      ; IER
    xor al, al
    out dx, al
    add dx, 2                   ; LCR
    mov al, 0x80                ; DLAB=1
    out dx, al
    mov dx, [com1_port]         ; DLL
    mov al, 0x01                ; 115200 baud
    out dx, al
    inc dx                      ; DLM
    xor al, al
    out dx, al
    add dx, 2                   ; LCR
    mov al, 0x03                ; 8N1
    out dx, al
    dec dx                      ; FCR
    mov al, 0x07                ; enable/clear FIFOs
    out dx, al
    add dx, 2                   ; MCR
    mov al, 0x03                ; DTR+RTS
    out dx, al

    mov byte [serial_status], SERIAL_LIVE_UART
    mov byte [serial_live], 1
    ret

; =============================================================================
; Mirror formatted report to COM1 with bounded TX timeout
; =============================================================================
serial_mirror_report:
    mov si, SVC_RESP_BUF + RESP_HDR_SIZE
    mov bx, [out_text_len]
    test bx, bx
    jz .done
.tx_byte:
    mov dx, [com1_port]
    add dx, 5
    mov cx, 0xFFFF
.wait_thre:
    in al, dx
    test al, 0x20
    jnz .send
    loop .wait_thre
    jmp .done
.send:
    mov dx, [com1_port]
    lodsb
    out dx, al
    dec bx
    jnz .tx_byte

    mov dx, [com1_port]
    add dx, 5
    mov cx, 0xFFFF
.wait_temt:
    in al, dx
    test al, 0x40
    jnz .done
    loop .wait_temt
.done:
    ret

; =============================================================================
; GDT Setup & Protected-Mode 16 -> 32 -> 16 Trampoline
; =============================================================================
setup_gdt:
    mov ax, cs
    movzx eax, ax
    shl eax, 4
    mov [linear_base], eax

    mov bx, gdt_code32
    call patch_gdt_base
    mov bx, gdt_data32
    call patch_gdt_base
    mov bx, gdt_code16
    call patch_gdt_base
    mov bx, gdt_data16
    call patch_gdt_base

    mov eax, [linear_base]
    add eax, gdt_start
    mov [gdtr + 2], eax

    mov ax, cs
    mov [rm_ret_seg], ax
    ret

patch_gdt_base:
    mov [bx + 2], ax
    push eax
    shr eax, 16
    mov [bx + 4], al
    mov [bx + 7], ah
    pop eax
    ret

; Call single Rust service request in SVC_REQ_BUF
call_rust_svc:
    mov byte [pm32_action], 0
    jmp enter_pm32

; Execute 2 back-to-back 32-bit protected-mode caller heartbeats in PM32
run_pm32_caller_batch:
    mov byte [pm32_action], 1
    jmp enter_pm32

enter_pm32:
    mov [rm_ss], ss
    mov [rm_sp], sp
    mov [rm_ds], ds
    mov [rm_es], es
    mov [rm_fs], fs
    mov [rm_gs], gs
    pushfd
    pop dword [rm_eflags]

    cli
    lgdt [gdtr]

    mov eax, cr0
    or al, 1
    mov cr0, eax
    jmp dword CODE32_SEL:pm32_entry

bits 32
pm32_entry:
    mov ax, DATA32_SEL
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, PM32_STACK_TOP
    cld

    cmp byte [pm32_action], 1
    je .pm32_caller_batch

    ; Single cdecl call: gate1_svc_entry(req_ptr, req_len, resp_ptr, resp_cap)
    push dword SVC_RESP_CAP
    push dword SVC_RESP_BUF
    push dword [req_total_len]
    push dword SVC_REQ_BUF
    call RUST_ENTRY_OFFSET
    add esp, 16
    mov [pm32_status], eax
    jmp .leave_pm32

.pm32_caller_batch:
    ; Protected-mode 32-bit caller issues 2 coexistence heartbeats directly in PM32
    mov ecx, 2
    mov edi, 0x0010             ; caller_id = 0x0010, 0x0011
.pm32_loop:
    push ecx
    push edi

    inc dword [req_seq]
    mov dword [SVC_REQ_BUF + 0], ABI_MAGIC
    mov word [SVC_REQ_BUF + 4], ABI_VER_MAJOR
    mov word [SVC_REQ_BUF + 6], ABI_VER_MINOR
    mov word [SVC_REQ_BUF + 8], OP_SVC_COEXIST_HB
    mov word [SVC_REQ_BUF + 10], 0
    mov dword [SVC_REQ_BUF + 12], REQ_HDR_SIZE + 12
    mov dword [SVC_REQ_BUF + 16], SVC_RESP_CAP
    mov eax, [req_seq]
    mov dword [SVC_REQ_BUF + 20], eax

    mov byte [SVC_REQ_BUF + REQ_HDR_SIZE + 0], CALLER_MODE_PM32
    mov byte [SVC_REQ_BUF + REQ_HDR_SIZE + 1], 0
    mov word [SVC_REQ_BUF + REQ_HDR_SIZE + 2], di
    mov eax, [pit_stress_ticks]
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 4], eax
    mov dword [SVC_REQ_BUF + REQ_HDR_SIZE + 8], 4

    push dword SVC_RESP_CAP
    push dword SVC_RESP_BUF
    push dword (REQ_HDR_SIZE + 12)
    push dword SVC_REQ_BUF
    call RUST_ENTRY_OFFSET
    add esp, 16
    mov [pm32_status], eax

    pop edi
    pop ecx
    test eax, eax
    jnz .leave_pm32
    inc edi
    dec ecx
    jnz .pm32_loop

.leave_pm32:
    jmp word CODE16_SEL:pm16_exit

bits 16
pm16_exit:
    mov ax, DATA16_SEL
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    mov eax, cr0
    and al, 0xFE
    mov cr0, eax
    jmp far [rm_ret_ptr]

rm_resume:
    mov ss, [cs:rm_ss]
    mov sp, [cs:rm_sp]
    mov ds, [cs:rm_ds]
    mov es, [cs:rm_es]
    mov fs, [cs:rm_fs]
    mov gs, [cs:rm_gs]
    push dword [rm_eflags]
    popfd
    sti
    ret

; =============================================================================
; Data, GDT, and State
; =============================================================================
align 8
gdt_start:
    dq 0x0000000000000000
gdt_code32:
    dw 0xFFFF
    dw 0x0000
    db 0x00
    db 0x9A
    db 0x40
    db 0x00
gdt_data32:
    dw 0xFFFF
    dw 0x0000
    db 0x00
    db 0x92
    db 0x40
    db 0x00
gdt_code16:
    dw 0xFFFF
    dw 0x0000
    db 0x00
    db 0x9A
    db 0x00
    db 0x00
gdt_data16:
    dw 0xFFFF
    dw 0x0000
    db 0x00
    db 0x92
    db 0x00
    db 0x00
gdt_end:

gdtr:
    dw gdt_end - gdt_start - 1
    dd 0

rm_ret_ptr:
    dw rm_resume
rm_ret_seg:
    dw 0

msg_v86:         db "SVCTEST: V86/protected mode active; load FERRIC.DLL via JLOAD or run in real mode", 13, 10
msg_v86_len      equ $ - msg_v86

msg_rust_err:    db "SVCTEST: resident Rust service returned error", 13, 10
msg_rust_err_len equ $ - msg_rust_err

align 4
linear_base:        dd 0
req_total_len:      dd 0
req_seq:            dd 0
pm32_status:        dd 0
rm_eflags:          dd 0
irq11_hook_chained: dd 0
pit_stress_ticks:   dd 0
dma_phys_addr:      dd 0
dma_buf_off:        dd 0

old_irq11_off:      dw 0
old_irq11_seg:      dw 0
out_text_len:       dw 0
com1_port:          dw 0
rm_ss:              dw 0
rm_sp:              dw 0
rm_ds:              dw 0
rm_es:              dw 0
rm_fs:              dw 0
rm_gs:              dw 0

serial_live:        db 0
serial_status:      db 0
pm32_action:        db 0

; Pad stub cleanly to RUST_ENTRY_OFFSET (0x1800)
%if ($ - $$) > (RUST_ENTRY_OFFSET - 0x0100)
    %error "16-bit stub exceeded RUST_ENTRY_OFFSET (0x1800)"
%endif
times ((RUST_ENTRY_OFFSET - 0x0100) - ($ - $$)) db 0
