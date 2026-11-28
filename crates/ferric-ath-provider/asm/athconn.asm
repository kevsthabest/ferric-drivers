; Experimental connector. Read 32 fresh random bytes from NONCE.BIN,
; prompt without echo for credentials; never pass secrets on command line.
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
 jc .allocfail
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
 ; Probe state before prompting or reading secrets.
 push cs
 pop es
 mov di,probe
 mov ax,0x0e
 call far [entry]
 jc .unready
 cmp byte [probe+1],4
 jne .unready
 mov ax,0x3d00
 mov dx,nonce_file
 int 0x21
 jc .invalid
 mov bx,ax
 mov dx,request+160
 mov cx,33
 mov ah,0x3f
 int 0x21
 pushf
 push ax
 mov ah,0x3e
 int 0x21
 pop ax
 popf
 jc .invalid
 cmp ax,32
 jne .invalid
 ; SSID, channel and BSSID input. Password is never echoed.
 mov dx,msg_ssid
 call puts
 mov di,request+16
 mov cx,32
 call line
 jc .invalid
 test ax,ax
 jz .invalid
 mov [request],al
 mov dx,msg_channel
 call puts
 mov di,small
 mov cx,2
 call line
 jc .invalid
 cmp ax,1
 jb .invalid
 cmp ax,2
 ja .invalid
 mov al,[small]
 sub al,'0'
 cmp al,9
 ja .invalid
 cmp byte [small+1],0
 je .channel
 cmp al,1
 jne .invalid
 mov al,[small+1]
 sub al,'0'
 cmp al,1
 ja .invalid
 add al,10
.channel:
 test al,al
 jz .invalid
 mov [request+3],al
 mov dx,msg_bssid
 call puts
 mov di,small
 mov cx,12
 call line
 jc .invalid
 cmp ax,12
 jne .invalid
 mov si,small
 mov di,request+4
 mov cx,6
.hexloop:
 lodsb
 call hex
 jc .invalid
 shl al,4
 mov bl,al
 lodsb
 call hex
 jc .invalid
 or al,bl
 stosb
 loop .hexloop
 mov dx,msg_pass
 call puts
 mov di,request+48
 mov cx,63
 call line
 jc .invalid
 cmp ax,8
 jb .invalid
 mov [request+1],al
 mov bx,1024
 mov ah,0x48
 int 0x21
 jc .allocfail
 mov [dma],ax
 mov bx,ax
 push cs
 pop es
 mov di,request
 mov ax,0x25
 call far [entry]
 jc .retain
 mov [status],ax
 test ax,ax
 jz .resident
 cmp ax,7
 je .retain
 mov es,[dma]
 mov ah,0x49
 int 0x21
 jc .retain
 jmp .exit
.retain:
 mov word [status],7
.resident:
 call wipe
 call report
 mov dx,(program_end-$$+0x100+15)/16
 mov ax,[status]
 mov ah,0x31
 int 0x21
.invalid:
 mov word [status],1
 jmp .exit
.unready:
 mov word [status],5
 jmp .exit
.allocfail:
 mov word [status],3
 jmp .exit
.missing:
 mov word [status],9
.exit:
 call wipe
 call report
 mov ax,[status]
 mov ah,0x4c
 int 0x21
; Read bounded line without echo; backspace supported, ESC aborts.
; Drain overlong line and reject rather than truncate.
line:
 push bx
 push dx
 push si
 push es
 push ds
 pop es
 mov si,di
 xor bx,bx
.next:
 mov ah,8
 int 0x21
 cmp al,13
 je .done
 cmp al,27
 je .bad
 cmp al,8
 je .back
 cmp al,32
 jb .bad
 cmp bx,cx
 jae .drain
 stosb
 inc bx
 jmp .next
.back:
 test bx,bx
 jz .next
 dec bx
 dec di
 mov byte [di],0
 jmp .next
.drain:
 mov ah,8
 int 0x21
 cmp al,13
 jne .drain
.bad:
 stc
 jmp .return
.done:
 mov ax,bx
 clc
.return:
 pushf
 push ax
 mov dx,crlf
 call puts
 pop ax
 popf
 pop es
 pop si
 pop dx
 pop bx
 ret
hex:
 cmp al,'0'
 jb .bad
 cmp al,'9'
 jbe .digit
 and al,0xdf
 cmp al,'A'
 jb .bad
 cmp al,'F'
 ja .bad
 sub al,'A'-10
 clc
 ret
.digit:
 sub al,'0'
 clc
 ret
.bad:
 stc
 ret
wipe:
 push es
 push ds
 pop es
 xor ax,ax
 mov di,request
 mov cx,193
 rep stosb
 pop es
 ret
puts:
 mov ah,9
 int 0x21
 ret
report:
 mov dx,msg_status
 call puts
 mov dl,[status]
 add dl,'0'
 mov ah,2
 int 0x21
 mov dx,crlf
 call puts
 ret
entry: dd 0
dma: dw 0
status: dw 0
probe: times 44 db 0
small: times 12 db 0
request: times 193 db 0
nonce_file: db 'NONCE.BIN',0
msg_ssid: db 'SSID (hidden input): $'
msg_channel: db 'Channel 1-11: $'
msg_bssid: db 'BSSID (12 hex digits): $'
msg_pass: db 'WPA2 passphrase (hidden): $'
msg_status: db 'ATHCONN: status=$'
crlf: db 13,10,'$'
align 2
times 512 db 0
stack_top:
program_end:
