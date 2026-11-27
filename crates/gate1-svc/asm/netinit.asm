; NETINIT.COM - install the Ferric packet driver (Gate 2 step 2.3).
;
; The DOS-facing half of the install lives here, in plain V86 DOS
; code: arena allocations, the INT 60h vector write, and staying
; resident are a DOS program's natural work, so this loader does
; them itself and asks the JLM only for ring-0 work. (Bring-up
; lesson, learned the hard way: INT 2Fh AX=1684h returns the API
; entry in ES:DI, not ES:BX. The first version of this loader took
; the offset from BX and far-called into garbage; the program died
; silently before printing anything, which sent the diagnosis down
; several wrong paths. G1LIVE had it right all along.)
;   API AX=0003h  probe NIC -> AX 0 present / 1 none, BX installed
;                 flag, CX packet-stub size in paragraphs
;   API AX=0005h  copy the packet stub into ES:DI -> AX byte length
;   API AX=0007h  record incumbent INT 60h vector (BX off, CX seg)
;   API AX=0006h  bring the NIC up (BX DMA seg, CX stub seg)
;                 -> AX 0 ok / 3 failed / 4 already installed
; The two allocated blocks and this loader all stay resident: the
; exit is INT 21h AH=31h, under which DOS frees none of the blocks
; owned by this PSP. Status: 0 installed, 1 no NIC, 2 alloc failed,
; 3 bring-up failed, 4 already installed, 9 no JLM.

bits 16
org 0x100

NET_DMA_PARAS equ 1024          ; 16 KiB; must match the Rust layout

start:
    ; Shrink our own block before allocating anything: DOS hands a
    ; .COM the largest free block, so without this the AH=48h calls
    ; below have no arena left to take from. (G1LIVE does the same;
    ; skipping it cost a debugging round - the allocs failed with a
    ; plain status 2 while hundreds of KB sat inside our own block.)
    mov bx, (netinit_end - $$ + 0x100 + 15) / 16
    mov ah, 0x4A
    int 0x21

    xor di, di
    mov es, di
    mov ax, 0x1684
    mov bx, 0xFD01
    int 0x2F
    mov [entry_off], di
    mov ax, es
    mov [entry_seg], ax
    or ax, di
    jnz .have_jlm
    mov word [status], 9
    jmp .report

.have_jlm:
    mov ax, 0x0003
    call far [entry_off]
    mov [api_cx], cx
    cmp bx, 0
    je .not_installed
    mov word [status], 4
    jmp .report
.not_installed:
    cmp ax, 0
    je .alloc_dma
    mov word [status], 1
    jmp .report

.alloc_dma:
    mov bx, NET_DMA_PARAS
    mov ah, 0x48
    int 0x21
    jnc .dma_ok
    mov word [status], 2
    jmp .report
.dma_ok:
    mov [dma_seg], ax
    mov bx, [api_cx]
    mov ah, 0x48
    int 0x21
    jnc .stub_ok
    mov word [status], 2
    jmp .free_report
.stub_ok:
    mov [stub_seg], ax

    ; The JLM copies the stub bytes into the block; then patch the
    ; stub's self-segment word at +0x0C so it can find its own data.
    mov ax, [stub_seg]
    mov es, ax
    xor di, di
    mov ax, 0x0005
    call far [entry_off]
    mov ax, [stub_seg]
    mov es, ax
    mov [es:0x000C], ax

    ; Save the incumbent INT 60h vector and hand it to the JLM for
    ; its detach path.
    mov ah, 0x35
    mov al, 0x60
    int 0x21
    mov [old_off], bx
    mov ax, es
    mov [old_seg], ax
    mov bx, [old_off]
    mov cx, [old_seg]
    mov ax, 0x0007
    call far [entry_off]

    ; Bring the NIC up on our DMA block.
    mov bx, [dma_seg]
    mov cx, [stub_seg]
    mov ax, 0x0006
    call far [entry_off]
    cmp ax, 0
    je .up
    cmp ax, 5
    je .quarantine
    mov word [status], 3
    jmp .free_report
.up:
    ; Point INT 60h at the stub (offset 0 in its block).
    push ds
    mov ax, [stub_seg]
    mov ds, ax
    xor dx, dx
    mov ax, 0x2560
    int 0x21
    pop ds

    mov word [status], 0
    call print_report
    ; Terminate-and-stay-resident: keeps this block and both
    ; allocated blocks (all owned by this PSP) in memory.
    mov dx, netinit_end
    add dx, 15
    shr dx, 4
    mov ax, 0x3100
    int 0x21

.quarantine:
    ; Bring-up may have started bus-master DMA. Keep both DOS blocks
    ; resident and leave INT 60h unchanged. Reboot before retrying.
    mov word [status], 5
    call print_report
    mov dx, netinit_end
    add dx, 15
    shr dx, 4
    mov ax, 0x3105
    int 0x21

.free_report:
    mov ax, [stub_seg]
    cmp ax, 0
    je .free_dma
    mov es, ax
    mov ah, 0x49
    int 0x21
.free_dma:
    mov ax, [dma_seg]
    cmp ax, 0
    je .report
    mov es, ax
    mov ah, 0x49
    int 0x21
.report:
    call print_report
    mov ax, [status]
    mov ah, 0x4C
    int 0x21

print_report:
    mov si, msg_status
    call print_str
    mov ax, [status]
    call print_dec16
    mov si, msg_crlf
    call print_str
    ret

print_dec16:
    push ax
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
    cmp ax, 0
    jne .div
.out:
    pop dx
    add dl, '0'
    mov ah, 0x02
    int 0x21
    loop .out
    pop dx
    pop cx
    pop bx
    pop ax
    ret

print_str:
    push ax
    mov ah, 0x09
    mov dx, si
    int 0x21
    pop ax
    ret

entry_off: dw 0
entry_seg: dw 0
api_cx:    dw 0
dma_seg:   dw 0
stub_seg:  dw 0
old_off:   dw 0
old_seg:   dw 0
status:    dw 0

msg_status: db 'NETINIT: status=$'
msg_crlf:   db 0x0D, 0x0A, '$'



netinit_end:
