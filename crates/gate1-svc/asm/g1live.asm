; Exercise the resident JLM from V86 mode and from a 32-bit DPMI client.
; Beyond the original version ping, the V86 phase now proves three more
; resident behaviours and reports them as separate lines:
;   GATE1:MEM          - largest free conventional block in this profile
;   GATE1:IRQHOOK      - the JLM's INT 73h chain hook observes a software
;                        INT 73h (telemetry total/chained both advance)
;   GATE1:DMARESIDENT  - OP_SVC_DMA_STRESS against a 4 KiB buffer in this
;                        client's own memory, validated by Jemm's VDMAD
; The DPMI phase still calls the V86 API through function 0301h.
cpu 386
bits 16
org 0x100

RESP_CAP        equ 256
OP_TELEMETRY    equ 0x0101
OP_DMA_STRESS   equ 0x0103
DMA_CRC_EXPECT  equ 0x0155C515

    mov sp, stack_top
    mov bx, (stack_top - $$ + 0x100 + 15) / 16
    mov ah, 0x4a
    int 0x21
    jc failed
    mov [myseg], ds

    xor di, di
    mov es, di
    mov ax, 0x1684
    mov bx, 0xfd01
    int 0x2f
    mov ax, es
    mov [rm_cs], ax
    mov [rm_ip], di
    or ax, di
    jz failed
    xor ax, ax
    xor bx, bx
    call far [rm_ip]
    jc failed
    cmp ax, 1
    jne failed
    cmp bx, 0xfd01
    jne failed

    ; ---- conventional memory: largest free block in the resident profile.
    ; The figure is reported, not gated: the connected-profile floor in
    ; the spec is provisional until Gate 2 publishes real footprints, so
    ; a shortfall here is data for that publication, not a Gate 1 failure.
    mov bx, 0xFFFF
    mov ah, 0x48
    int 0x21                    ; always CF; BX = largest block (paragraphs)
    movzx eax, bx
    shl eax, 4
    mov di, mem_line_num
    call hex32

    ; ---- IRQ hook proof: telemetry, one software INT 73h, telemetry.
    mov word [req_opcode], OP_TELEMETRY
    mov dword [req_len32], 24
    mov dword [req_seq], 1
    call build_header
    mov word [req_len_w], 24
    call call_dispatch
    test eax, eax
    jnz .irq_done
    mov eax, [resp_buf + 24]    ; payload total_invocations
    mov [tel_total0], eax
    mov eax, [resp_buf + 32]    ; payload chained_events
    mov [tel_chained0], eax

    int 0x73                    ; Jemm's monitor routes hooked V86 vectors
                                ; through the chain hooks before the IVT

    mov dword [req_seq], 2
    call build_header
    mov word [req_len_w], 24
    call call_dispatch
    test eax, eax
    jnz .irq_done
    mov eax, [resp_buf + 24]
    mov [tel_total1], eax
    mov di, irq_line_total
    call hex32
    mov eax, [resp_buf + 32]
    mov [tel_chained1], eax
    mov di, irq_line_chained
    call hex32
    mov eax, [tel_total1]
    cmp eax, [tel_total0]
    jbe .irq_done
    mov eax, [tel_chained1]
    cmp eax, [tel_chained0]
    jbe .irq_done
    mov byte [irq_ok], 1
    mov byte [irq_line_status], 'P'
    mov byte [irq_line_status + 1], 'A'
    mov byte [irq_line_status + 2], 'S'
    mov byte [irq_line_status + 3], 'S'
.irq_done:

    ; ---- DMA stress through the resident dispatch.
    ; The bounce buffer lives in this program's own memory; its linear
    ; address is the physical address because conventional memory is
    ; identity-mapped under JEMMEX, and the resident core now proves
    ; that against Jemm's page tables via VDMAD before touching a byte.
    mov ax, ds
    movzx eax, ax
    shl eax, 4
    add eax, dma_area
    add eax, 4095
    and eax, 0xFFFFF000
    mov [dma_lin], eax

    mov word [req_opcode], OP_DMA_STRESS
    mov dword [req_len32], 48
    mov dword [req_seq], 3
    call build_header
    mov eax, [dma_lin]
    mov [req_buf + 24], eax             ; desc.phys_addr
    mov dword [req_buf + 28], 4096      ; desc.capacity
    mov dword [req_buf + 32], 4096      ; desc.len
    mov dword [req_buf + 36], 4096      ; desc.align
    mov dword [req_buf + 40], 3         ; flags: BELOW_1M | NO_64K_CROSS
    mov [req_buf + 44], eax             ; buf_offset = same linear address
    mov word [req_len_w], 48
    call call_dispatch
    test eax, eax
    jnz .dma_done
    cmp dword [resp_buf + 8], 0         ; response header status
    jne .dma_done
    mov eax, [resp_buf + 24 + 20]       ; result.final_crc32
    mov di, dma_line_crc
    call hex32
    cmp eax, DMA_CRC_EXPECT
    jne .dma_done
    cmp byte [resp_buf + 24 + 18], 1    ; integrity_ok
    jne .dma_done
    cmp byte [resp_buf + 24 + 17], 3    ; passes_completed
    jne .dma_done
    mov al, [resp_buf + 24 + 15]        ; neg_tests_passed
    cmp al, [resp_buf + 24 + 16]        ; neg_tests_total
    jne .dma_done
    mov byte [dma_ok], 1
    mov byte [dma_line_status], 'P'
    mov byte [dma_line_status + 1], 'A'
    mov byte [dma_line_status + 2], 'S'
    mov byte [dma_line_status + 3], 'S'
.dma_done:

    ; ---- DPMI phase (unchanged coexistence proof).
    mov ax, 0x1687
    int 0x2f
    test ax, ax
    jnz failed
    test bl, 1
    jz failed
    mov [dpmi_entry], di
    mov [dpmi_entry + 2], es
    test si, si
    jz enter_dpmi
    mov bx, si
    mov ah, 0x48
    int 0x21
    jc failed
    mov es, ax
enter_dpmi:
    mov ax, 1
    call far [dpmi_entry]
    jc failed_pm
    ; Initial CS retains 16-bit instruction defaults. The 32-bit client
    ; supplies ES:EDI to DPMI. A zero SS:SP requests the host's RM stack.
    push ds
    pop es
    mov edi, rm_regs
    xor bx, bx
    xor cx, cx
    mov ax, 0x0301
    int 0x31
    jc failed_pm
    test byte [rm_flags], 1
    jnz failed_pm
    cmp word [rm_eax], 1
    jne failed_pm
    cmp word [rm_ebx], 0xfd01
    jne failed_pm
    cmp byte [irq_ok], 1
    jne failed_pm
    cmp byte [dma_ok], 1
    jne failed_pm
    mov dx, pass_msg
    xor al, al
    jmp print_pm
failed_pm:
    mov dx, fail_msg
    mov al, 1
print_pm:
    ; The verdict lines are reported through the DPMI translation
    ; service, not a bare INT 21h: HDPMI32i mistranslates the DS:DX
    ; string pointer of a protected-mode AH=09h call and page-faults
    ; in ring 0 (reproduced one-variable from a working client on
    ; 2026-11-20). INT 31h AX=0300h runs each print in V86, where DOS
    ; sees the string in this client's own conventional-memory segment.
    mov [pm_exit], al
    mov [main_msg], dx
    mov dx, mem_line
    call print_line_pm
    mov dx, irq_line
    call print_line_pm
    mov dx, dma_line
    call print_line_pm
    mov dx, [main_msg]
    call print_line_pm
    mov al, [pm_exit]
    mov ah, 0x4c
    int 0x21

; print_line_pm: DX = offset of a $-terminated string in our segment.
print_line_pm:
    mov word [rm_print_eax], 0x0900
    mov [rm_print_edx], dx
    mov ax, [myseg]
    mov [rm_print_ds], ax
    push ds
    pop es
    mov edi, rm_print
    mov bl, 0x21
    xor bh, bh
    xor cx, cx
    mov ax, 0x0300
    int 0x31
    ret

failed:
    mov dx, fail_msg
    mov ah, 9
    int 0x21
    mov ax, 0x4c01
    int 0x21

; build_header: fill req_buf's 24-byte header from req_opcode/req_len32/req_seq.
build_header:
    mov dword [req_buf], 0x534F4446     ; "FDOS"
    mov word [req_buf + 4], 0           ; version major
    mov word [req_buf + 6], 1           ; version minor
    mov ax, [req_opcode]
    mov [req_buf + 8], ax
    mov word [req_buf + 10], 0          ; flags
    mov eax, [req_len32]
    mov [req_buf + 12], eax
    mov dword [req_buf + 16], RESP_CAP
    mov eax, [req_seq]
    mov [req_buf + 20], eax
    ret

; call_dispatch: AX=1 far call to the JLM V86 API proc.
;   DS:SI = req_buf, CX = req_len_w, ES:DI = resp_buf, DX = RESP_CAP.
;   Returns EAX = the service's StatusCode (0 = Ok).
call_dispatch:
    push ds
    pop es
    mov ax, 1
    mov si, req_buf
    mov cx, [req_len_w]
    mov di, resp_buf
    mov dx, RESP_CAP
    call far [rm_ip]
    ret

; hex32: write EAX as 8 uppercase hex digits at DS:DI; advances DI.
hex32:
    push cx
    push edx
    mov cx, 8
.next:
    rol eax, 4
    mov edx, eax
    and edx, 0xF
    mov dl, [hexdigits + edx]
    mov [di], dl
    inc di
    loop .next
    pop edx
    pop cx
    ret
hexdigits db '0123456789ABCDEF'

dpmi_entry dd 0
rm_regs:
    times 4 dd 0            ; EDI, ESI, EBP, reserved
rm_ebx dd 0
    times 2 dd 0            ; EDX, ECX
rm_eax dd 0
rm_flags dw 0
    times 4 dw 0            ; ES, DS, FS, GS
rm_ip dw 0
rm_cs dw 0
    dw 0, 0                 ; SP, SS
myseg dw 0
pm_exit db 0
main_msg dw 0
req_opcode dw 0
req_len32 dd 0
req_seq dd 0
req_len_w dw 0
tel_total0 dd 0
tel_chained0 dd 0
tel_total1 dd 0
tel_chained1 dd 0
dma_lin dd 0
irq_ok db 0
dma_ok db 0
req_buf times 64 db 0
resp_buf times RESP_CAP db 0
rm_print:
    times 4 dd 0            ; EDI, ESI, EBP, reserved
    dd 0                    ; EBX
rm_print_edx dd 0
    dd 0                    ; ECX
rm_print_eax dd 0
    dw 0                    ; flags
    dw 0                    ; ES
rm_print_ds dw 0
    dw 0, 0, 0, 0           ; FS, GS, IP, CS
    dw 0, 0                 ; SP, SS
pass_msg db 'GATE1:LIVE PASS V86 and DPMI called FERRIC.JLM',13,10,'$'
fail_msg db 'GATE1:LIVE FAIL resident service or DPMI call',13,10,'$'
mem_line db 'GATE1:MEM largest=0x'
mem_line_num db '00000000'
    db ' budget=0x00091000',13,10,'$'
irq_line db 'GATE1:IRQHOOK status='
irq_line_status db 'FAIL'
    db ' total=0x'
irq_line_total db '00000000'
    db ' chained=0x'
irq_line_chained db '00000000'
    db 13,10,'$'
dma_line db 'GATE1:DMARESIDENT status='
dma_line_status db 'FAIL'
    db ' crc=0x'
dma_line_crc db '00000000'
    db 13,10,'$'
align 4
dma_area times 8192 db 0
    times 1024 db 0
stack_top:
