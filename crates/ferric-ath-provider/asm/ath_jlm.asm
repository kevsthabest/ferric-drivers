; Experimental standalone AR2413 consumer FD0A. No activation on load.
cpu 386
bits 32
extern ath_version,ath_probe,ath_shutdown,ath_query
extern ath_packet_api,ath_packet_ready,ath_packet_busy,ath_upcall_next,ath_upcall_after
extern ath_stub_validate
%define PKT_BRIDGE_VECTOR 0xF1
%define TIMER_VECTOR 0x1C
%define VMM_SIMULATE_FAR_RET 13
%define VMM_SIMULATE_FAR_CALL 11
%define VMM_BEGIN_NEST_EXEC 23
%define VMM_RESUME_EXEC 25
%define VMM_END_NEST_EXEC 26
%define VMM_HOOK_V86_INT_CHAIN 4
%define VMM_UNHOOK_V86_INT_CHAIN 0x24
%define VMM_GET_DDB 48
%define CLIENT_EDI 0
%define CLIENT_ESI 4
%define CLIENT_EBX 16
%define CLIENT_ECX 24
%define CLIENT_EAX 28
%define CLIENT_EFLAGS 48
%define CLIENT_ES 60
%define CLIENT_DS 64

%define VMM_PAGE_FREE 0x12
%define VMM_PAGE_RESERVE 0x27
%define VMM_PAGE_COMMIT_PHYS 0x2A
%define PR_SYSTEM 0x80080000
%define PC_MMIO_FLAGS 0x40120000
%macro VMMCall 1
 int 0x20
 dw %1,1
%endmacro
%macro FRESCall 1
 int 0x20
 dw %1,0xFD07
%endmacro
%macro CBCall 1
 int 0x20
 dw %1,0xFD09
%endmacro
section .data align=4
services: dd ath_version
_pkt_oldvec: dd 0
timer_oldvec: dd 0
bridge_hooked: dd 0
timer_hooked: dd 0
claim60: dd 0
claimf1: dd 0
net_in_upcall: dd 0
net_upcall_buf: times 8 dd 0
net_saved_client: times 19 dd 0
net_budget: dd 0
stub_segment: dd 0
old_vector: dd 0
stub_start: incbin "out/athstub.bin"
stub_end:
global _ddb
_ddb:
 dd 0
 dw 0x0100,0xFD0A
 db 1,0
 dw 0
 db "ATH5K   "
 dd 0x80000000,control,dispatch
 dd 0,0,0,0
 dd services,1
 dd 0,0,80,0,0,0
section .text align=4
global ath_map
ath_map:
    push ebx
    push esi
    mov ebx, [esp + 12]
    mov esi, [esp + 16]
    or esi, esi
    jz .map_fail
    push dword 0
    push esi
    push dword PR_SYSTEM
    VMMCall VMM_PAGE_RESERVE
    add esp, 12
    cmp eax, 0xFFFFFFFF
    je .map_fail
    or eax, eax
    jz .map_fail
    push eax
    mov ecx, eax
    shr ecx, 12
    mov edx, ebx
    shr edx, 12
    push dword PC_MMIO_FLAGS
    push edx
    push esi
    push ecx
    VMMCall VMM_PAGE_COMMIT_PHYS
    add esp, 16
    or eax, eax
    pop eax
    jz .commit_fail
    pop esi
    pop ebx
    ret
.commit_fail:
    ; Release the reserved linear pages when PageCommitPhys failed.
    push dword 0
    push eax
    VMMCall VMM_PAGE_FREE
    add esp, 8
.map_fail:
    xor eax, eax
    pop esi
    pop ebx
    ret

global ath_unmap
ath_unmap:
    ; cdecl: PageFree takes the original linear base, flags=0.
    push dword 0
    push dword [esp + 8]
    VMMCall VMM_PAGE_FREE
    add esp, 8
    ret



global ath_cb_query,ath_cb_acquire,ath_cb_release,ath_cb_master
%macro CHECK_CB 0
 push edi
 xor edi,edi
 mov eax,0xFD09
 VMMCall 48
 pop edi
 test ecx,ecx
 jz cb_missing
 cmp dword [ecx+48],0
 je cb_missing
 cmp dword [ecx+52],5
 jb cb_missing
%endmacro
cb_missing:
 mov eax,0xffffffff
 ret
ath_cb_query:
 CHECK_CB
 times 2 push dword [esp+8]
 CBCall 1
 add esp,8
 ret
ath_cb_acquire:
 CHECK_CB
 push dword 0xFD0A
 push dword [esp+8]
 CBCall 2
 add esp,8
 ret
ath_cb_release:
 CHECK_CB
 push dword 0xFD0A
 push dword [esp+8]
 CBCall 3
 add esp,8
 ret
ath_cb_master:
 CHECK_CB
 push dword [esp+8]
 push dword 0xFD0A
 push dword [esp+12]
 CBCall 4
 add esp,12
 ret
control:
 clc
 ret
dispatch:
 pushfd
 cli
 cld
 VMMCall 13
 and dword [ebp+48],~1
 movzx eax,word [ebp+28]
 test eax,eax
 jz .ping
 cmp eax,0x0a
 je .probe
 cmp eax,0x0d
 je .stop
 cmp eax,0x0e
 je .query
 cmp eax,0x20
 je .stub_info
 cmp eax,0x21
 je .stub_copy
 cmp eax,0x22
 je .install
 cmp eax,0x23
 je .remove
 jmp .bad
.ping:
 mov word [ebp+28],1
 mov word [ebp+16],0xFD0A
 jmp .done
.probe:
 ; AX=0A, BX=slot. No external pointer.
 movzx eax,word [ebp+16]
 push eax
 call ath_probe
 add esp,4
 mov [ebp+28],eax
 jmp .done
.query:
 movzx eax,word [ebp+60]
 shl eax,4
 movzx edx,word [ebp]
 cmp edx,0xffd4
 ja .bad
 add eax,edx
 cmp eax,0x500
 jb .bad
 cmp eax,0xfffd4
 ja .bad
 push eax
 call ath_query
 add esp,4
 mov [ebp+28],eax
 jmp .done
.stub_info:
 mov word [ebp+CLIENT_EAX],stub_end-stub_start
 mov word [ebp+CLIENT_ECX],(stub_end-stub_start+15)/16
 jmp .done
.stub_copy:
 movzx eax,word [ebp+CLIENT_ES]
 cmp eax,0x50
 jb .bad
 cmp eax,0x9fe0
 ja .bad
 cmp word [ebp+CLIENT_EDI],0
 jne .bad
 cmp word [ebp+CLIENT_ECX],stub_end-stub_start
 jb .bad
 push eax
 call ath_stub_validate
 add esp,4
 ; Validate returns 1 for valid memory regardless of existing content.
 test eax,eax
 jz .bad
 movzx edi,word [ebp+CLIENT_ES]
 shl edi,4
 mov esi,stub_start
 mov ecx,stub_end-stub_start
 rep movsb
 mov word [ebp+CLIENT_EAX],stub_end-stub_start
 jmp .done
.install:
 call packet_install
 mov [ebp+CLIENT_EAX],eax
 jmp .done
.remove:
 call packet_remove
 mov [ebp+CLIENT_EAX],eax
 jmp .done
.stop:
 call packet_remove
 test eax,eax
 jnz .stop_result
 call ath_shutdown
.stop_result:
 mov [ebp+28],eax
 jmp .done
.bad:
 or dword [ebp+48],1
 mov dword [ebp+28],0
.done:
 popfd
 ret
; Install only after connected, with a retained DOS stub. Claim and
; hook incrementally; failed rollback pins state for explicit retry.
packet_install:
    cmp dword [stub_segment],0
    jne .busy
    cmp dword [claim60],0
    jne .busy
    call ath_packet_ready
    test eax,eax
    jz .not_ready
    movzx eax,word [ebp+CLIENT_EBX]
    cmp eax,0x50
    jb .invalid
    cmp eax,0x9fe0
    ja .invalid
    push eax
    call ath_stub_validate
    add esp,4
    test eax,eax
    jz .invalid
    movzx eax,word [ebp+CLIENT_EBX]
    shl eax,4
    cmp dword [eax+3],0x20544B50
    jne .invalid
    cmp dword [eax+7],0x52565244
    jne .invalid
    mov dx,word [ebp+CLIENT_EBX]
    cmp [eax+0x0c],dx
    jne .invalid
    ; Snapshot incumbent vector, refuse an existing packet driver.
    mov eax,[0x180]
    mov [old_vector],eax
    mov edx,eax
    shr eax,16
    shl eax,4
    and edx,0xffff
    cmp edx,0xfff4
    ja .invalid
    add eax,edx
    cmp eax,0xffff4
    ja .invalid
    cmp dword [eax+3],0x20544B50
    jne .claim
    cmp dword [eax+7],0x52565244
    je .busy
.claim:
    push dword 0
    push dword 0x60
    push dword 5
    push dword 0xfd0a
    FRESCall 3
    add esp,16
    test eax,eax
    jnz .busy
    mov dword [claim60],1
    push dword 0
    push dword PKT_BRIDGE_VECTOR
    push dword 5
    push dword 0xfd0a
    FRESCall 3
    add esp,16
    test eax,eax
    jnz .rollback
    mov dword [claimf1],1
    mov eax,PKT_BRIDGE_VECTOR
    mov esi,_pkt_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    jc .rollback
    mov dword [bridge_hooked],1
    mov eax,TIMER_VECTOR
    mov esi,timer_hook
    VMMCall VMM_HOOK_V86_INT_CHAIN
    jc .rollback
    mov dword [timer_hooked],1
    movzx eax,word [ebp+CLIENT_EBX]
    mov [stub_segment],eax
    shl eax,16
    mov [0x180],eax
    xor eax,eax
    ret
.rollback:
    call packet_remove
    test eax,eax
    jnz .retained
.busy:
    mov eax,2
    ret
.not_ready:
    mov eax,5
    ret
.invalid:
    mov eax,1
    ret
.retained:
    mov eax,7
    ret
packet_remove:
    cmp dword [net_in_upcall],0
    jne .fail
    call ath_packet_busy
    test eax,eax
    jnz .fail
    mov eax,[stub_segment]
    test eax,eax
    jz .timer
    shl eax,16
    ; Refuse if another owner replaced our vector.
    cmp [0x180],eax
    jne .fail
    mov eax,[old_vector]
    mov [0x180],eax
    mov eax,[stub_segment]
    shl eax,4
    mov dword [eax],0x89550BB6
    mov dword [eax+4],0x064E83E5
    mov dword [eax+8],0x90CF5D01
    mov dword [stub_segment],0
.timer:
    cmp dword [timer_hooked],0
    je .bridge
    mov eax,TIMER_VECTOR
    mov esi,timer_hook
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    jc .fail
    mov dword [timer_hooked],0
.bridge:
    cmp dword [bridge_hooked],0
    je .f1
    mov eax,PKT_BRIDGE_VECTOR
    mov esi,_pkt_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    jc .fail
    mov dword [bridge_hooked],0
.f1:
    cmp dword [claimf1],0
    je .v60
    push dword 0
    push dword PKT_BRIDGE_VECTOR
    push dword 5
    push dword 0xfd0a
    FRESCall 4
    add esp,16
    test eax,eax
    jnz .fail
    mov dword [claimf1],0
.v60:
    cmp dword [claim60],0
    je .done
    push dword 0
    push dword 0x60
    push dword 5
    push dword 0xfd0a
    FRESCall 4
    add esp,16
    test eax,eax
    jnz .fail
    mov dword [claim60],0
.done:
    xor eax,eax
    ret
.fail:
    mov eax,7
    ret
 dd _pkt_oldvec
_pkt_hookproc:
    cmp dword [stub_segment],0
    je .chain
    pushfd
    cli
    cld
    pushad
    push ebp
    call ath_packet_api
    add esp,4
    popad
    popfd
    clc
    ret
.chain:
    stc
    ret
 dd timer_oldvec
timer_hook:
    pushfd
    cli
    cld
    pushad
    cmp dword [stub_segment],0
    je .done
    call _net_drain_upcalls
.done:
    popad
    popfd
    stc
    ret

_net_drain_upcalls:
    cmp dword [net_in_upcall], 0
    jne .net_upcalls_reentered
    mov dword [net_in_upcall], 1
    mov dword [net_budget], 16
.net_upcall_loop:
    cmp dword [net_budget],0
    je .net_upcalls_done
    dec dword [net_budget]
    push net_upcall_buf
    call ath_upcall_next
    add esp, 4
    or eax, eax
    jz .net_upcalls_done
    push esi
    push edi
    mov esi, ebp
    mov edi, net_saved_client
    mov ecx, 19
    rep movsd
    pop edi
    pop esi
    VMMCall VMM_BEGIN_NEST_EXEC
    cmp dword [net_upcall_buf + 4], 0
    jne .net_phase1
    mov dword [ebp + CLIENT_EAX], 0     ; phase 0: offer a buffer
    jmp .net_set_common
.net_phase1:
    mov dword [ebp + CLIENT_EAX], 1     ; phase 1: frame is in the buffer
    mov eax, [net_upcall_buf + 16]
    mov [ebp + CLIENT_DS], eax
    mov [ebp + CLIENT_ES], eax
    mov eax, [net_upcall_buf + 20]
    mov [ebp + CLIENT_ESI], eax
    mov [ebp + CLIENT_EDI], eax
.net_set_common:
    mov eax, [net_upcall_buf + 12]
    mov [ebp + CLIENT_EBX], eax
    mov eax, [net_upcall_buf + 8]
    mov [ebp + CLIENT_ECX], eax
    cmp dword [net_upcall_buf + 4], 0
    jne .net_nest_go
    mov eax, [net_upcall_buf]           ; phase 0: DS = receiver segment
    shr eax, 16
    mov [ebp + CLIENT_DS], eax
.net_nest_go:
    mov ecx, [net_upcall_buf]
    mov edx, ecx
    and edx, 0xFFFF                     ; offset
    shr ecx, 16                         ; segment
    VMMCall VMM_SIMULATE_FAR_CALL
    VMMCall VMM_RESUME_EXEC
    VMMCall VMM_END_NEST_EXEC
    cli
    cld
    cmp dword [net_upcall_buf + 4], 0
    jne .net_after
    mov eax, [ebp + CLIENT_ES]          ; phase 0 answer: offered buffer
    and eax, 0xFFFF
    mov [net_upcall_buf + 16], eax
    mov eax, [ebp + CLIENT_EDI]
    and eax, 0xFFFF
    mov [net_upcall_buf + 20], eax
.net_after:
    push esi
    push edi
    mov esi, net_saved_client
    mov edi, ebp
    mov ecx, 19
    rep movsd
    pop edi
    pop esi
    push net_upcall_buf
    call ath_upcall_after
    add esp, 4
    jmp .net_upcall_loop
.net_upcalls_done:
    mov dword [net_in_upcall], 0
.net_upcalls_reentered:
    ret


global DllMain
DllMain:
 mov eax,[esp+8]
 pushfd
 cli
 push ebx
 push esi
 push edi
 push ebp
 cld
 cmp eax,1
 jne .detach
 xor edi,edi
 mov eax,0xFD01
 VMMCall 48
 test ecx,ecx
 jnz .fail
 xor edi,edi
 mov eax,0xFD09
 VMMCall 48
 test ecx,ecx
 jz .fail
 cmp dword [ecx+48],0
 je .fail
 cmp dword [ecx+52],5
 jb .fail
 CBCall 0
 cmp eax,0x10000
 jne .fail
 xor edi,edi
 mov eax,0xFD07
 VMMCall 48
 test ecx,ecx
 jz .fail
 cmp dword [ecx+48],0
 je .fail
 cmp dword [ecx+52],5
 jb .fail
 FRESCall 0
 cmp eax,0x10000
 jne .fail
 push dword 0xFD0A
 FRESCall 1
 add esp,4
 test eax,eax
 jnz .fail
 jmp .ok
.detach:
 cmp eax,0
 jne .ok
 call packet_remove
 test eax,eax
 jnz .fail
 call ath_shutdown
 test eax,eax
 jnz .fail
 push dword 0xFD0A
 FRESCall 2
 add esp,4
 test eax,eax
 jnz .fail
.ok:
 mov eax,1
 jmp .return
.fail:
 xor eax,eax
.return:
 pop ebp
 pop edi
 pop esi
 pop ebx
 popfd
 ret 12
