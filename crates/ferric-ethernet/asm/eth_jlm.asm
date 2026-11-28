; Standalone Intel 8255x packet driver, DDB FD08.
cpu 386
bits 32
%define ETH_ID 0xFD08
%define FRES_ID 0xFD07
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
%macro VMMCall 1
 int 0x20
 dw %1, 1
%endmacro
%macro FRESCall 1
 int 0x20
 dw %1, FRES_ID
%endmacro
extern gate1_net_probe, gate1_net_init, gate1_net_stub_info, gate1_net_stub_copy
extern gate1_net_api, gate1_net_service, gate1_net_upcall_next, gate1_net_upcall_after
extern eth_irq, eth_poll, eth_shutdown, eth_release_resources
section .data align=4
_pkt_oldvec: dd 0
irq_oldvec: dd 0
timer_oldvec: dd 0
irq_hooked: dd 0
irq_vector: dd 0
timer_hooked: dd 0
bridge_hooked: dd 0
net_upcall_buf: times 8 dd 0
net_saved_client: times 19 dd 0
net_stub_info: times 3 dd 0
net_stub_seg: dd 0
net_dma_seg: dd 0
net_old_vec60: dd 0
net_old_vec60_saved: dd 0
net_in_upcall: dd 0
net_installed: dd 0
global _ddb
_ddb:
 dd 0
 dw 0x0100, ETH_ID
 db 1,0
 dw 0
 db "I8255X  "
 dd 0x80000000, control, dispatch
 dd 0,0,0,0,0,0,0,0,80,0,0,0
section .text align=4
control:
 clc
 ret
global eth_claim
eth_claim:
 push dword [esp+12]
 push dword [esp+12]
 push dword [esp+12]
 push dword ETH_ID
 FRESCall 3
 add esp,16
 ret
global eth_release
eth_release:
 push dword [esp+12]
 push dword [esp+12]
 push dword [esp+12]
 push dword ETH_ID
 FRESCall 4
 add esp,16
 ret
dispatch:
 VMMCall VMM_SIMULATE_FAR_RET
 and dword [ebp+CLIENT_EFLAGS], ~1
 movzx eax, word [ebp+CLIENT_EAX]
 cmp eax,0
 je .version
 cmp eax,3
 je _v86_net_probe_info
 cmp eax,5
 je _v86_net_stub_copy
 cmp eax,6
 je _v86_net_init_call
 cmp eax,7
 je _v86_net_note_oldvec
 or dword [ebp+CLIENT_EFLAGS],1
 ret
.version:
 mov word [ebp+CLIENT_EAX],1
 mov word [ebp+CLIENT_EBX],ETH_ID
 ret
_v86_net_probe_info:
    push net_stub_info
    call gate1_net_stub_info
    add esp, 4
    mov eax, [net_stub_info + 8]
    mov [ebp + CLIENT_ECX], eax         ; stub paragraphs
    mov eax, [net_installed]
    mov [ebp + CLIENT_EBX], eax
    call gate1_net_probe
    or eax, eax
    jnz .npi_present
    mov word [ebp + CLIENT_EAX], 1
    ret
.npi_present:
    mov word [ebp + CLIENT_EAX], 0
    ret

; AX=0x0005: copy the packet stub into the caller's buffer at ES:DI.
_v86_net_stub_copy:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    push eax
    cmp edx, 0xFE00
    ja .bad_copy
    cmp eax, 0xFFE00
    ja .bad_copy
    test eax, eax
    jz .bad_copy
    call gate1_net_stub_copy
    add esp, 4
    mov [ebp + CLIENT_EAX], eax
    ret
.bad_copy:
    add esp, 4
    mov dword [ebp + CLIENT_EAX], 0
    ret

; AX=0x0006: bring the NIC up on NETINIT's DMA block and hook the
; INT F1h bridge. The INT 60h vector itself is NETINIT's job.
_v86_net_init_call:
    cmp dword [net_installed], 0
    je .nic_go
    mov word [ebp + CLIENT_EAX], 4
    ret
.nic_go:
    movzx eax, word [ebp + CLIENT_EBX]
    mov [net_dma_seg], eax
    movzx eax, word [ebp + CLIENT_ECX]
    mov [net_stub_seg], eax
    cmp dword [irq_hooked], 0
    jne .irq_ready
    call eth_irq
    cmp eax, 8
    jb .master_irq
    add eax, 0x68
    jmp .hook_irq
.master_irq:
    add eax, 8
.hook_irq:
    mov [irq_vector], eax
    mov esi, irq_hook
    VMMCall VMM_HOOK_V86_INT_CHAIN
    jc .nic_fail
    mov dword [irq_hooked], 1
.irq_ready:
    push dword [net_dma_seg]
    call gate1_net_init
    add esp, 4
    cmp eax, 1
    je .nic_up
    cmp eax, 2
    jne .nic_fail
    mov dword [net_installed], 2 ; quarantined allocation
    mov word [ebp + CLIENT_EAX], 5 ; retain DMA until reboot
    ret
.nic_fail:
    mov word [ebp + CLIENT_EAX], 3
    ret
.nic_up:
    mov dword [net_installed], 1
    mov word [ebp + CLIENT_EAX], 0
    ret

; AX=0x0007: remember the incumbent INT 60h vector for detach.
; Only the first installer records the pre-Ferric vector so a later WCONN
; after NETINIT (or a reconnect) never overwrites it with our own stub.
_v86_net_note_oldvec:
    cmp dword [net_old_vec60_saved], 0
    jne .note_vec60_done
    movzx eax, word [ebp + CLIENT_ECX]
    shl eax, 16
    movzx edx, word [ebp + CLIENT_EBX]
    or eax, edx
    mov [net_old_vec60], eax
    mov dword [net_old_vec60_saved], 1
.note_vec60_done:
    mov word [ebp + CLIENT_EAX], 0
    ret


 dd _pkt_oldvec
_pkt_hookproc:
 pushad
 push ebp
 call gate1_net_api
 add esp,4
 popad
 clc
 ret
 dd irq_oldvec
irq_hook:
 pushad
 call gate1_net_service
 test eax,eax
 jz .chain
 call _net_drain_upcalls
 mov al,0x20
 cmp dword [irq_vector],0x70
 jb .master
 out 0xA0,al
.master:
 out 0x20,al
 popad
 clc
 ret
.chain:
 popad
 stc
 ret
 dd timer_oldvec
timer_hook:
 pushad
 call eth_poll
 call _net_drain_upcalls
 popad
 stc
 ret
_net_drain_upcalls:
    cmp dword [net_in_upcall], 0
    jne .net_upcalls_reentered
    mov dword [net_in_upcall], 1
.net_upcall_loop:
    push net_upcall_buf
    call gate1_net_upcall_next
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
    call gate1_net_upcall_after
    add esp, 4
    jmp .net_upcall_loop
.net_upcalls_done:
    mov dword [net_in_upcall], 0
.net_upcalls_reentered:
    ret


global DllMain
DllMain:
 cmp dword [esp+8],1
 jne .detach
 push edi
 xor edi,edi
 mov eax,0xFD01
 VMMCall VMM_GET_DDB
 test ecx,ecx
 jnz .attach_fail
 xor edi,edi
 mov eax,FRES_ID
 VMMCall VMM_GET_DDB
 test ecx,ecx
 jz .attach_fail
 cmp dword [ecx+48],0
 je .attach_fail
 cmp dword [ecx+52],5
 jb .attach_fail
 FRESCall 0
 cmp eax,0x10000
 jne .attach_fail
 push dword ETH_ID
 FRESCall 1
 add esp,4
 test eax,eax
 jnz .attach_fail
 ; Claim INT 60h and the private bridge exclusively within FRES.
 push dword 0
 push dword 0x60
 push dword 5
 call eth_claim
 add esp,12
 test eax,eax
 jnz .unregister
 push dword 0
 push dword PKT_BRIDGE_VECTOR
 push dword 5
 call eth_claim
 add esp,12
 test eax,eax
 jnz .release60
 mov eax,PKT_BRIDGE_VECTOR
 mov esi,_pkt_hookproc
 VMMCall VMM_HOOK_V86_INT_CHAIN
 jc .release_bridge
 mov dword [bridge_hooked],1
 mov eax,TIMER_VECTOR
 mov esi,timer_hook
 VMMCall VMM_HOOK_V86_INT_CHAIN
 jc .unhook_bridge
 mov dword [timer_hooked],1
 pop edi
 mov eax,1
 ret 12
.unhook_bridge:
 mov eax,PKT_BRIDGE_VECTOR
 mov esi,_pkt_hookproc
 VMMCall VMM_UNHOOK_V86_INT_CHAIN
.release_bridge:
 push dword 0
 push dword PKT_BRIDGE_VECTOR
 push dword 5
 call eth_release
 add esp,12
.release60:
 push dword 0
 push dword 0x60
 push dword 5
 call eth_release
 add esp,12
.unregister:
 push dword ETH_ID
 FRESCall 2
 add esp,4
.attach_fail:
 pop edi
.fail:
 xor eax,eax
 ret 12
.detach:
 cmp dword [esp+8],0
 jne .ok
 cmp dword [net_in_upcall],0
 jne .fail
 call eth_shutdown
 test eax,eax
 jnz .fail
 ; Restore the public vector only if it still points at our stub.
 cmp dword [net_installed],1
 jne .no_vector
 mov eax,[net_stub_seg]
 shl eax,16
 cmp [0x180],eax
 jne .no_vector
 mov eax,[net_old_vec60]
 mov [0x180],eax
.no_vector:
 cmp dword [net_installed],0
 je .no_stub
 mov eax,[net_stub_seg]
 shl eax,4
 test eax,eax
 jz .no_stub
 mov byte [eax],0xCF
.no_stub:
 cmp dword [irq_hooked],0
 je .no_irq
 mov eax,[irq_vector]
 mov esi,irq_hook
 VMMCall VMM_UNHOOK_V86_INT_CHAIN
 jc .fail
 mov dword [irq_hooked],0
.no_irq:
 cmp dword [timer_hooked],0
 je .no_timer
 mov eax,TIMER_VECTOR
 mov esi,timer_hook
 VMMCall VMM_UNHOOK_V86_INT_CHAIN
 jc .fail
 mov dword [timer_hooked],0
.no_timer:
 cmp dword [bridge_hooked],0
 je .no_bridge
 mov eax,PKT_BRIDGE_VECTOR
 mov esi,_pkt_hookproc
 VMMCall VMM_UNHOOK_V86_INT_CHAIN
 jc .fail
 mov dword [bridge_hooked],0
.no_bridge:
 call eth_release_resources
 test eax,eax
 jnz .fail
 push dword 0
 push dword PKT_BRIDGE_VECTOR
 push dword 5
 call eth_release
 add esp,12
 test eax,eax
 jnz .fail
 push dword 0
 push dword 0x60
 push dword 5
 call eth_release
 add esp,12
 test eax,eax
 jnz .fail
 push dword ETH_ID
 FRESCall 2
 add esp,4
 test eax,eax
 jnz .fail
.ok:
 mov eax,1
 ret 12
