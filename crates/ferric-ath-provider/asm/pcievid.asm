; PCIEVID.COM: read-only PCI mechanism #1 inventory, buses 0..4.
; No BAR sizing writes, no configuration data writes, no MMIO or DMA.
; Every CF8 access restores the original latch and interrupt flags.
cpu 386
bits 16
org 100h
start:
 push cs
 pop ds
 cld
 mov dx,title
 call puts
 mov byte [bus],0
.bus:
 mov byte [dev],0
.dev:
 mov byte [fn],0
.fn:
 xor si,si
 call read_cfg
 cmp ax,0xffff
 je .next_fn
 test ax,ax
 jz .next_fn
 mov dx,record
 call puts
 mov al,[bus]
 call hex8
 mov dl,':'
 call char
 mov al,[dev]
 call hex8
 mov dl,'.'
 call char
 mov al,[fn]
 call hex8
 mov dx,crlf
 call puts
 xor si,si
.row:
 mov ax,si
 call hex8
 mov dl,':'
 call char
 mov cx,4
.col:
 mov dl,' '
 call char
 call read_cfg
 call hex32
 add si,4
 loop .col
 mov dx,crlf
 call puts
 cmp si,256
 jb .row
.next_fn:
 inc byte [fn]
 cmp byte [fn],8
 jb .fn
 inc byte [dev]
 cmp byte [dev],32
 jb .dev
 inc byte [bus]
 cmp byte [bus],5
 jb .bus
 mov dx,done
 call puts
 mov ax,4c00h
 int 21h
; SI config dword offset. EAX result; preserve CX and DX.
read_cfg:
 push ebx
 push ecx
 push edx
 pushfd
 cli
 mov dx,0xcf8
 in eax,dx
 push eax
 movzx eax,byte [bus]
 shl eax,16
 movzx ebx,byte [dev]
 shl ebx,11
 or eax,ebx
 movzx ebx,byte [fn]
 shl ebx,8
 or eax,ebx
 movzx ebx,si
 or eax,ebx
 or eax,0x80000000
 out dx,eax
 mov dx,0xcfc
 in eax,dx
 mov ecx,eax
 pop eax
 mov dx,0xcf8
 out dx,eax
 mov eax,ecx
 popfd
 pop edx
 pop ecx
 pop ebx
 ret
hex32:
 push eax
 push cx
 mov cx,8
.loop:
 rol eax,4
 push eax
 and al,15
 call nibble
 pop eax
 loop .loop
 pop cx
 pop eax
 ret
hex8:
 push ax
 shr al,4
 call nibble
 pop ax
 and al,15
 call nibble
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
 push ax
 push bx
 push cx
 push dx
 push si
 mov ah,2
 int 21h
 pop si
 pop dx
 pop cx
 pop bx
 pop ax
 ret
puts:
 push ax
 push bx
 push cx
 push dx
 push si
 mov ah,9
 int 21h
 pop si
 pop dx
 pop cx
 pop bx
 pop ax
 ret
bus: db 0
dev: db 0
fn: db 0
title: db 'PCIEVID: BEGIN buses=00-04 readonly=1',13,10,'$'
record: db 'PCI BDF=','$'
crlf: db 13,10,'$'
done: db 'PCIEVID: END',13,10,'$'
