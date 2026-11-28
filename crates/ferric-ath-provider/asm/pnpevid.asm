; PNPEVID.COM - DynaBook BIOS 1.20 PnP static device-node capture.
; Uses only standard read/query functions 0 and 1, control=2 (boot config).
; No raw SMI, setters, PCI writes, socket activation, or current-config query.
cpu 386
bits 16
org 100h
%define MAX_NODE 8192
start:
 push cs
 pop ds
 cld
 mov dx,title
 call puts
 smsw ax
 test ax,1
 jnz protected_mode
 cmp sp,program_end+1024
 jb short_stack
 ; Match the captured $PnP installation structure before any firmware call.
 mov ax,0xf000
 mov es,ax
 mov bx,0x0280
 cmp dword [es:bx],0x506e5024
 jne wrong_bios
 cmp word [es:bx+4],0x2110
 jne wrong_bios
 cmp word [es:bx+0x0d],0x08db
 jne wrong_bios
 cmp word [es:bx+0x0f],0xf000
 jne wrong_bios
 cmp word [es:bx+0x1b],0
 jne wrong_bios
 cmp dword [es:bx+0x17],0x9937f351
 jne wrong_bios
 mov si,bx
 mov cx,33
 xor al,al
.header_sum:
 add al,[es:si]
 inc si
 loop .header_sum
 test al,al
 jnz wrong_bios
 mov ax,[es:bx+0x0d]
 mov [entry],ax
 mov ax,[es:bx+0x0f]
 mov [entry+2],ax
 mov ax,[es:bx+0x1b]
 mov [selector],ax
 mov dx,header_ok
 call puts
 call get_count
 mov dx,fn0_msg
 call puts
 mov ax,[bios_rc]
 call hex16
 mov dx,crlf
 call puts
 cmp word [bios_rc],0
 jne bios_error
 mov al,[node_count]
 mov [remaining],al
 mov dx,count_msg
 call puts
 call hex8
 mov dx,max_msg
 call puts
 mov ax,[node_max]
 call hex16
 mov dx,crlf
 call puts
 cmp byte [remaining],0
 je bad_count
 cmp word [node_max],12
 jb bad_count
 cmp word [node_max],MAX_NODE
 ja bad_count
.next_node:
 mov al,[next_id]
 mov [requested],al
 ; Full static buffer in the program allocation; no external address access.
 push ds
 pop es
 mov di,node_buf
 mov cx,MAX_NODE
 mov al,0xcc
 cld
 rep stosb
 call get_node
 cmp word [bios_rc],0
 jne bios_error
 cmp dword [guard_lo],0x13579bdf
 jne bad_node
 cmp dword [guard_hi],0x2468ace0
 jne bad_node
 mov ax,[node_buf]
 cmp ax,12
 jb bad_node
 cmp ax,[node_max]
 ja bad_node
 movzx bx,byte [node_buf+2]
 cmp bl,0xff
 je bad_node
 cmp byte [seen+bx],0
 jne bad_node
 mov byte [seen+bx],1
 mov al,[requested]
 test al,al
 jz .handle_ok
 cmp al,bl
 jne bad_node
.handle_ok:
 mov dx,node_msg
 call puts
 mov al,[requested]
 call hex8
 mov dx,handle_msg
 call puts
 mov al,[node_buf+2]
 call hex8
 mov dx,next_msg
 call puts
 mov al,[next_id]
 call hex8
 mov dx,size_msg
 call puts
 mov ax,[node_buf]
 call hex16
 mov dx,crlf
 call puts
 xor si,si
 mov cx,[node_buf]
.row:
 mov ax,si
 call hex16
 mov dl,':'
 call char
 mov bx,16
.byte:
 mov dl,' '
 call char
 mov al,[node_buf+si]
 call hex8
 inc si
 dec cx
 jz .record_done
 dec bx
 jnz .byte
 mov dx,crlf
 call puts
 jmp .row
.record_done:
 mov dx,crlf
 call puts
 dec byte [remaining]
 jz .last
 cmp byte [next_id],0xff
 je incomplete
 movzx bx,byte [next_id]
 cmp byte [seen+bx],0
 jne bad_node
 jmp .next_node
.last:
 cmp byte [next_id],0xff
 jne incomplete
 xor al,al
 jmp finish
protected_mode:
 mov al,1
 jmp finish
wrong_bios:
 mov al,2
 jmp finish
bad_count:
 mov al,3
 jmp finish
bios_error:
 mov dx,err_msg
 call puts
 mov ax,[bios_rc]
 call hex16
 mov dx,crlf
 call puts
 mov al,4
 jmp finish
bad_node:
 mov al,5
 jmp finish
short_stack:
 mov al,6
 jmp finish
incomplete:
 mov al,7
finish:
 mov [status],al
 mov dx,done_msg
 call puts
 mov al,[status]
 call hex8
 mov dx,crlf
 call puts
 mov al,[status]
 mov ah,0x4c
 int 0x21

; PnP BIOS 1.0a section 4.5.1/4.5.2: 16-bit C far-call ABI.
; BIOS owns its selector; pass the actual value from the validated header.
%macro save_regs 0
 pushf
 pusha
 push ds
 push es
 push fs
 push gs
%endmacro
%macro restore_regs 0
 pop gs
 pop fs
 pop es
 pop ds
 popa
 popf
 cld
 ret
%endmacro
get_count:
 save_regs
 push word [selector]
 push cs
 push word node_max
 push cs
 push word node_count
 push word 0
 call far [cs:entry]
 add sp,12
 mov [cs:bios_rc],ax
 restore_regs
get_node:
 save_regs
 push word [selector]
 push word 2
 push cs
 push word node_buf
 push cs
 push word next_id
 push word 1
 call far [cs:entry]
 add sp,14
 mov [cs:bios_rc],ax
 restore_regs
hex16:
 push ax
 mov al,ah
 call hex8
 pop ax
 call hex8
 ret
hex8:
 push ax
 mov ah,al
 shr al,4
 call nibble
 mov al,ah
 and al,15
 call nibble
 pop ax
 ret
nibble:
 add al,'0'
 cmp al,'9'
 jbe .digit
 add al,7
.digit:
 mov dl,al
 call char
 ret
char:
 pusha
 push ds
 push es
 mov ah,2
 int 21h
 pop es
 pop ds
 popa
 cld
 ret
puts:
 pusha
 push ds
 push es
 mov ah,9
 int 21h
 pop es
 pop ds
 popa
 cld
 ret
entry: dd 0
selector: dw 0
bios_rc: dw 0
node_count: dw 0
node_max: dw 0
next_id: db 0
requested: db 0
remaining: db 0
status: db 0
title: db 'PNPEVID: BEGIN static_boot=1',13,10,'$'
header_ok: db 'PNPEVID: header=F000:0280 verified',13,10,'$'
fn0_msg: db 'PNPEVID: fn0=','$'
count_msg: db 'PNPEVID: nodes=','$'
max_msg: db ' max=','$'
node_msg: db 'NODE request=','$'
handle_msg: db ' handle=','$'
next_msg: db ' next=','$'
size_msg: db ' size=','$'
err_msg: db 'PNPEVID: bios_rc=','$'
done_msg: db 'PNPEVID: END status=','$'
crlf: db 13,10,'$'
seen: times 256 db 0
guard_lo: dd 0x13579bdf
node_buf: times MAX_NODE db 0
guard_hi: dd 0x2468ace0
program_end:
