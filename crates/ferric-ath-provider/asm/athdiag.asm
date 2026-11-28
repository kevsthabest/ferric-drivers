; Read-only standalone diagnostic plus explicit probe/release requests.
cpu 386
bits 16
org 0x100
start:
 push cs
 pop ds
 mov ax,0x1684
 mov bx,0xfd09
 xor di,di
 mov es,di
 int 0x2f
 mov [cbentry],di
 mov [cbentry+2],es
 mov ax,es
 or ax,di
 jz .nocb
 mov ax,8
 call far [cbentry]
 mov si,msg_cb
 call report
 xor bx,bx
.loop:
 push bx
 mov ax,0x0a
 call far [cbentry]
 mov si,msg_audit
 call report
 pop bx
 inc bx
 cmp bx,2
 jb .loop
 jmp .ath
.nocb:
 mov ax,9
 mov si,msg_cb
 call report
.ath:
 mov ax,0x1684
 mov bx,0xfd0a
 xor di,di
 mov es,di
 int 0x2f
 mov [athentry],di
 mov [athentry+2],es
 mov ax,es
 or ax,di
 jnz .found
 mov ax,9
 mov si,msg_ath
 call report
 mov ax,0x4c09
 int 0x21
.found:
 xor ax,ax
 call far [athentry]
 cmp bx,0xfd0a
 jne .wrong
 cmp ax,1
 jne .wrong
 xor ax,ax
 mov si,msg_ath
 call report
 xor bx,bx
 mov ax,0x0a
 call far [athentry]
 mov si,msg_probe
 call report
 push cs
 pop es
 mov di,info
 mov ax,0x0e
 call far [athentry]
 jc .wrong
 mov si,msg_stage
 xor ax,ax
 mov al,[info+1]
 call report
 mov ax,0x20
 call far [athentry]
 jc .wrong
 mov si,msg_stub
 call report
 xor bx,bx
 mov ax,0x22
 call far [athentry]
 mov si,msg_install
 call report
 mov ax,0x0d
 call far [athentry]
 mov si,msg_stop
 call report
 mov ah,0x4c
 int 0x21
.wrong:
 mov ax,0x4c01
 int 0x21
report:
 push ax
 mov dx,si
 mov ah,9
 int 0x21
 pop ax
 push ax
 push bx
 push cx
 push dx
 xor cx,cx
 mov bx,10
.div:
 xor dx,dx
 div bx
 push dx
 inc cx
 test ax,ax
 jnz .div
.digits:
 pop dx
 add dl,'0'
 mov ah,2
 int 0x21
 loop .digits
 mov dx,crlf
 mov ah,9
 int 0x21
 pop dx
 pop cx
 pop bx
 pop ax
 ret
cbentry: dd 0
athentry: dd 0
info: times 44 db 0
msg_cb: db 'ATHDIAG: bridges=$'
msg_audit: db 'ATHDIAG: audit=$'
msg_ath: db 'ATHDIAG: resident=$'
msg_probe: db 'ATHDIAG: probe=$'
msg_stage: db 'ATHDIAG: stage=$'
msg_stub: db 'ATHDIAG: stub_bytes=$'
msg_install: db 'ATHDIAG: install=$'
msg_stop: db 'ATHDIAG: stop=$'
crlf: db 13,10,'$'
