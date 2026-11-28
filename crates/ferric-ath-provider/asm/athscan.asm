; Standalone passive scan client. Retain DOS arena on uncertain DMA stop.
cpu 386
bits 16
org 0x100
start:
 mov sp,stack_top
 push cs
 pop ds
 push cs
 pop es
 mov bx,(program_end-$$+0x100+15)/16
 mov ah,0x4a
 int 0x21
 jc .alloc_fail
 mov ax,0x1684
 mov bx,0xfd0a
 xor di,di
 mov es,di
 int 0x2f
 mov [entry],di
 mov [entry+2],es
 mov ax,es
 or ax,di
 jz .missing
 mov bx,1024
 mov ah,0x48
 int 0x21
 jc .alloc_fail
 mov [dma],ax
 mov bx,ax
 push cs
 pop es
 mov di,result
 mov ax,0x24
 call far [entry]
 jc .retain
 cmp ax,7
 je .retain
 mov [status],ax
 mov es,[dma]
 mov ah,0x49
 int 0x21
 jc .retain
 jmp .exit
.retain:
 mov word [status],7
 call report
 mov dx,(program_end-$$+0x100+15)/16
 mov ax,0x3107
 int 0x21
.missing:
 mov word [status],9
 jmp .exit
.alloc_fail:
 mov word [status],3
.exit:
 call report
 cmp word [status],0
 jne .done
 mov cl,[result+2]
 xor ch,ch
 cmp cx,16
 ja .done
 mov si,result+16
.next:
 jcxz .done
 push cx
 push si
 mov cl,[si+10]
 xor ch,ch
 cmp cx,32
 ja .skip
 mov dx,si
 add dx,12
 mov bx,1
 mov ah,0x40
 int 0x21
 mov dx,crlf
 mov ah,9
 int 0x21
.skip:
 pop si
 pop cx
 add si,44
 loop .next
.done:
 mov ax,[status]
 mov ah,0x4c
 int 0x21
report:
 mov dx,msg
 mov ah,9
 int 0x21
 mov dl,[status]
 add dl,'0'
 mov ah,2
 int 0x21
 mov dx,crlf
 mov ah,9
 int 0x21
 ret
entry: dd 0
dma: dw 0
status: dw 0
result: times 720 db 0
msg: db 'ATHSCAN: status=$'
crlf: db 13,10,'$'
align 2
times 512 db 0
stack_top:
program_end:
