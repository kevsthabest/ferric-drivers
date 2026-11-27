; =============================================================================
; Ferric DOS Gate 1 Track B - JLM PE32 Resident Module Stub (FERRIC.DLL)
; =============================================================================
; Assembled with NASM (`-f win32`) and linked with OpenWatcom `wlink`
; (`format windows nt dll runtime native`) against the freestanding 32-bit
; i386-unknown-none Rust `gate1-svc` core to produce a relocatable PE32 JLM
; compatible with JEMM386 + JLOAD (see vendor/ref/jemm/Include/JLM.INC and
; vendor/ref/jemm/JLM/GENERIC/GENERIC.ASM).
;
; Strictly 386 baseline (`cpu 386`), read-only, ZERO DOS INT 21h calls.
; =============================================================================

cpu 386
bits 32

; JLM / VMM constants verified against vendor/ref/jemm/Include/JLM.INC
GATE1_DEVICE_ID         equ 0xFD01
DLL_PROCESS_ATTACH      equ 1
VMM_GET_DDB             equ 48
DLL_PROCESS_DETACH      equ 0
VMM_SIMULATE_FAR_RET    equ 13          ; VMM_Service Simulate_Far_Ret (0x0D in JLM.INC:94)
VMM_SIMULATE_FAR_CALL   equ 11          ; VMM_Service Simulate_Far_Call (JLM.INC order: 11)
VMM_BEGIN_NEST_EXEC     equ 23          ; VMM_Service Begin_Nest_Exec
VMM_RESUME_EXEC         equ 25          ; VMM_Service Resume_Exec
VMM_END_NEST_EXEC       equ 26          ; VMM_Service End_Nest_Exec
MOUSE_IRQ_VECTOR        equ 0x74        ; slave PIC IRQ 12 as a V86 interrupt vector
INT33_VECTOR            equ 0x33        ; DOS mouse API software interrupt
INT15_VECTOR            equ 0x15        ; BIOS system services (AH=84h joystick support)
JOY_GAMEPORT            equ 0x201       ; Standard IBM PC Gameport I/O port
VMM_HOOK_V86_INT_CHAIN  equ 4           ; VMM_Service Hook_V86_Int_Chain (JLM.INC:85)
VMM_INSTALL_IO_HANDLER  equ 0x1F        ; VMM_Service Install_IO_Handler (JLM.INC:112)
VMM_REMOVE_IO_HANDLER   equ 0x22        ; VMM_Service Remove_IO_Handler (JLM.INC:115)
VMM_UNHOOK_V86_INT_CHAIN equ 0x24       ; VMM_Service Unhook_V86_Int_Chain (JLM.INC:117)
SB_IRQ5_VECTOR          equ 0x0D        ; master PIC IRQ 5 (Sound Blaster Pro)
IRQ11_VECTOR            equ 0x73        ; slave PIC IRQ 11 as a V86 interrupt vector
TIMER_IRQ_VECTOR        equ 0x1C        ; V86 user timer tick (called 18.2 Hz by BIOS INT 08h)
VMM_EXEC_INT            equ 24          ; VMM_Service Exec_Int (JLM.INC order: 24)
VMM_PAGE_RESERVE        equ 0x27        ; VMM_Service _PageReserve (JLM.INC:120)
VMM_PAGE_COMMIT_PHYS    equ 0x2A        ; VMM_Service _PageCommitPhys (JLM.INC:123)
PR_SYSTEM               equ 0x80080000  ; _PageReserve system linear arena
PC_MMIO_FLAGS           equ 0x40120000  ; PC_WRITEABLE | PC_CACHEDIS | PC_INCR
CLIENT_EDI              equ 0
CLIENT_ESI              equ 4
CLIENT_DS               equ 64
PKT_BRIDGE_VECTOR       equ 0xF1        ; stub -> JLM bridge interrupt
MOUSE_BRIDGE_VECTOR     equ 0xF2        ; DPMI 0300h INT 33h -> JLM bridge interrupt
MOUSE_IRQ_BRIDGE_VECTOR equ 0xF3        ; IVT[0x74] (IRQ 12) -> JLM bridge interrupt
PKT_INT_VECTOR          equ 0x60        ; packet-driver vector (stub lives here)
NET_DMA_PARAS           equ 1024        ; 16 KiB packet-driver DMA block
DDB_STRUCT_SIZE         equ 80          ; sizeof(VxD_Desc_Block) in JLM.INC

; Client_Reg_Struc offsets (vendor/ref/jemm/Include/JLM.INC lines 282-302)
CLIENT_EDI              equ 0
CLIENT_ESI              equ 4
CLIENT_EBP              equ 8
CLIENT_EBX              equ 16
CLIENT_EDX              equ 20
CLIENT_ECX              equ 24
CLIENT_EAX              equ 28
CLIENT_EFLAGS           equ 48
CLIENT_ES               equ 60
CLIENT_DS               equ 64

; JEMM VMMCall macro: INT 20h followed by word service number and word device 1
%macro VMMCall 1
    int 0x20
    dw %1
    dw 1
%endmacro

extern gate1_svc_entry
extern gate1_irq11_isr
extern gate1_svc_reset
extern gate1_svc_set_platform_vdmad
extern gate1_mouse_init
extern gate1_mouse_feed
extern gate1_mouse_api
extern gate1_mouse_cb_info
extern gate1_net_probe
extern gate1_net_init
extern gate1_net_stub_info
extern gate1_net_stub_copy
extern gate1_net_api
extern gate1_net_service
extern gate1_net_poll_wifi
extern gate1_net_upcall_next
extern gate1_net_upcall_after
extern gate1_cb_init
extern gate1_cb_slot_info
extern gate1_wifi_probe
extern gate1_wifi_scan
extern gate1_wifi_connect
extern gate1_wifi_diag
extern gate1_net_switch_backend
extern gate1_wifi_disconnect
extern gate1_usb_init
extern gate1_usb_shutdown
extern gate1_usb_poll
extern gate1_usb_set_delay
extern gate1_usb_get_delay
extern gate1_usb_mouse_take_wheel
extern gate1_joy_io
extern gate1_joy_present
extern gate1_joy_int15
extern gate1_usb_msc_dir
extern gate1_usb_msc_open
extern gate1_usb_msc_read_chunk
extern gate1_usb_msc_bpb
extern gate1_usb_msc_read_sec
extern gate1_usb_msc_write_sec
extern gate1_usb_msc_flush
extern gate1_usb_msc_eject
extern gate1_usb_msc_media_check
extern gate1_blk_abi_version
extern gate1_blk_info
extern gate1_usb_msc_fs_type
extern gate1_ehci_recon
extern gate1_ehci_bringup
extern gate1_ehci_shutdown
extern gate1_m7101_info
extern gate1_m7101_set_throttle
extern gate1_m7101_set_backlight
extern gate1_sd_init
extern gate1_sd_read_sec
extern gate1_sd_shutdown
extern gate1_sf2_begin
extern gate1_sf2_write_chunk
extern gate1_sf2_commit
extern gate1_sf2_unload
extern gate1_sf2_status
extern gate1_audio_init
extern gate1_audio_external_prepare
extern gate1_audio_chime
extern gate1_audio_status
extern gate1_audio_shutdown
extern gate1_audio_sb_start
extern gate1_audio_irq11
extern gate1_audio_poll
extern gate1_audio_set_vol
extern gate1_sb_io
extern gate1_sb_take_irq
extern gate1_sb_take_up_to
extern gate1_sb_take_one_irq
extern gate1_sb_diag
extern gate1_sb_dma_diag
extern gate1_sb_mixer_ctl

section .data align=4

; Storage for the previous INT 73h hook in Jemm's chain. The dword
; immediately before _irq11_hookproc holds this variable's address;
; Hook_V86_Int_Chain writes the displaced hook address here and
; CallV86Hooks follows it whenever our proc returns with CF set.
_irq11_oldvec:
    dd 0
_timer_oldvec:
    dd 0

; Same chain-hook storage contract for the two Gate 2 mouse hooks.
_mouse_irq_oldvec:
    dd 0
_int33_oldvec:
    dd 0
_int15_oldvec:
    dd 0
joy_hooks_installed:
    dd 0

; Callback dispatch buffer filled by gate1_mouse_cb_info:
; [addr (seg<<16|off), report_mask, buttons, x, y, mickey_x, mickey_y]
mouse_cb_info_buf:
    times 7 dd 0
; Saved copy of the Client_Reg_Struc (19 dwords) taken before a
; callback nest and restored after it (see _int33_hookproc).
mouse_in_cb:
    dd 0
mouse_saved_client:
    times 19 dd 0
mouse_hooks_installed:
    dd 0
mouse_irq_installed:
    dd 0
mouse_ivt_patched:
    dd 0
mouse_old_ivt33:
    dd 0
mouse_old_ivt74:
    dd 0
_mouse_bridge_oldvec:
    dd 0
sb_traps_installed:
    dd 0
sb_in_irq:
    dd 0
sb_saved_client:
    times 19 dd 0
; _sb_deliver_pending_irq gets its own nest-save buffer instead of
; sharing sb_saved_client with _sb_io_handler: EOI is sent before
; delivery, so a fresh IRQ 11 can re-enter the delivery while an
; I/O trap (or an outer delivery) is still mid-nest, and two sites
; writing one save buffer corrupt the outer restore.
sb_deliv_saved_client:
    times 19 dd 0

; Packet driver (Gate 2 step 2.3): chain storage for the INT 0F1h
; bridge hook, the upcall sequencer's in/out buffer
; [addr, phase, len, handle, buf_seg, buf_off, 0, 0], a saved client
; block for the upcall nest (same discipline as mouse_saved_client),
; and the install state NETINIT fills in.
_pkt_oldvec:
    dd 0
net_upcall_buf:
    times 8 dd 0
net_saved_client:
    times 19 dd 0
net_stub_info:
    times 3 dd 0
net_stub_seg:
    dd 0
net_dma_seg:
    dd 0
net_old_vec60:
    dd 0
net_old_vec60_saved:
    dd 0
net_in_upcall:
    dd 0
net_installed:
    dd 0
wifi_saved_esp:
    dd 0
    align 16
wifi_r0_stack:
    times 8192 db 0
wifi_r0_stack_top:
    align 16
audio_r0_stack:
    times 2048 db 0
audio_r0_stack_top:

; Stage F portable Block-Transport DDB Service Table (device 0xFD01, 6 services).
; Callable from standalone filesystem JLMs (EXFAT.JLM 0xFD02, NTFS.JLM 0xFD03, etc.)
; via JEMM native `int 0x20; dd (0xFD01 << 16) | svc_idx`.
align 4
_ferric_blk_service_table:
    dd gate1_blk_abi_version        ; 0: BLK_SVC_ABI_VERSION
    dd gate1_blk_info               ; 1: BLK_SVC_DEV_INFO
    dd gate1_usb_msc_read_sec       ; 2: BLK_SVC_READ_SEC
    dd gate1_usb_msc_write_sec      ; 3: BLK_SVC_WRITE_SEC
    dd gate1_usb_msc_flush          ; 4: BLK_SVC_FLUSH
    dd gate1_usb_msc_media_check    ; 5: BLK_SVC_MEDIA_CHECK

; Exported VxD_Desc_Block (DDB) at ordinal 1 for JLOAD device registration.
; Layout matches VxD_Desc_Block in vendor/ref/jemm/Include/JLM.INC (80 bytes).
global _ddb
_ddb:
    dd 0                    ; +00 DDB_Next
    dw 0x0100               ; +04 DDB_Version (1.0)
    dw GATE1_DEVICE_ID      ; +06 DDB_Req_Device_Number (0xFD01)
    db 0                    ; +08 DDB_Dev_Major_Version
    db 1                    ; +09 DDB_Dev_Minor_Version
    dw 0                    ; +10 DDB_Flags
    db "FERRIC  "           ; +12 DDB_Name (8 bytes, space padded)
    dd 0x80000000           ; +20 DDB_Init_Order (UNDEFINED_INIT_ORDER)
    dd _jlm_control_proc    ; +24 DDB_Control_Proc
    dd _v86_dispatch        ; +28 DDB_V86_API_Proc
    dd _pm_dispatch         ; +32 DDB_PM_API_Proc
    dd 0                    ; +36 DDB_V86_API_CSIP
    dd 0                    ; +40 DDB_PM_API_CSIP
    dd 0                    ; +44 DDB_Reference_Data
    dd _ferric_blk_service_table ; +48 DDB_Service_Table_Ptr
    dd 6                         ; +52 DDB_Service_Table_Size
    dd 0                    ; +56 DDB_Win32_Service_Table
    dd 0                    ; +60 DDB_Prev
    dd DDB_STRUCT_SIZE      ; +64 DDB_Size (80)
    dd 0                    ; +68 DDB_Reserved1 (PE module handle)
    dd 0                    ; +72 DDB_Reserved2
    dd 0                    ; +76 DDB_Reserved3

section .text align=4

; JLM control message handler (SYSTEM_EXIT / DEVICE_REBOOT_NOTIFY).
; Returns CF=0 on success.
_jlm_control_proc:
    clc
    ret

; V86-mode API dispatcher (invoked via INT 2Fh AX=1684h BX=0xFD01 far call).
; Client registers in [ebp]:
;   AX = 0x0000: Get JLM/ABI version -> AX = 0x0001 (ABI 0.1), BX = 0xFD01
;   AX = 0x0001: Dispatch ABI request:
;                DS:SI = request buffer (flat linear = (DS << 4) + SI), CX = req_len
;                ES:DI = response buffer (flat linear = (ES << 4) + DI), DX = resp_cap
;                Returns EAX = StatusCode in Client_EAX
;   AX = 0x0002: Shared IRQ 11 ISR entry (EBX = status, CL = irq, EDX = tick)
;   AX = 0x0003: Packet-driver probe/info (Gate 2 step 2.3) ->
;                AX = 0 present / 1 no NIC, BX = already-installed flag,
;                CX = packet stub size in paragraphs.
;   AX = 0x0005: Copy the packet stub into the caller's ES:DI buffer ->
;                AX = stub length in bytes.
;   AX = 0x0006: Bring the NIC up (BX = DMA block segment, CX = stub
;                block segment) -> AX = 0 ok / 3 failed / 4 installed.
;   AX = 0x0007: Record the incumbent INT 60h vector (BX = offset,
;                CX = segment) for the detach path -> AX = 0.
;   The install sequence itself (DOS allocations, the INT 60h vector
;   write, the resident exit) lives in NETINIT.COM, in plain V86 DOS
;   code: those are a DOS program's natural jobs, and this proc's
;   part of the install is only the ring-0 work below.
_v86_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1     ; clear CF

    movzx eax, word [ebp + CLIENT_EAX]
    cmp eax, 0
    je .get_version
    cmp eax, 1
    je .dispatch_v86
    cmp eax, 2
    je .irq11_call
    cmp eax, 3
    je _v86_net_probe_info
    cmp eax, 5
    je _v86_net_stub_copy
    cmp eax, 6
    je _v86_net_init_call
    cmp eax, 7
    je _v86_net_note_oldvec
    cmp eax, 8
    je _v86_cb_init
    cmp eax, 9
    je _v86_cb_slot_info
    cmp eax, 10
    je _v86_wifi_probe
    cmp eax, 11
    je _v86_wifi_scan
    cmp eax, 12
    je _v86_wifi_connect
    cmp eax, 13
    je _v86_wifi_diag
    cmp eax, 14
    je _v86_net_switch
    cmp eax, 15
    je _v86_wifi_disconnect
    cmp eax, 16
    je _v86_usb_init
    cmp eax, 17
    je _v86_usb_shutdown
    cmp eax, 18
    je _v86_usb_msc_dir
    cmp eax, 19
    je _v86_usb_msc_open
    cmp eax, 20
    je _v86_usb_msc_read_chunk
    cmp eax, 21
    je _v86_usb_msc_bpb
    cmp eax, 22
    je _v86_usb_msc_read_sec
    cmp eax, 23
    je _v86_usb_msc_write_sec
    cmp eax, 24
    je _v86_usb_msc_flush
    cmp eax, 25
    je _v86_usb_msc_eject
    cmp eax, 26
    je _v86_usb_msc_media_check
    cmp eax, 0x1B
    je _v86_usb_delay
    cmp eax, 0x1C
    je _v86_usb_mouse_wheel
    cmp eax, 0x35
    je _v86_usb_fs_type
    cmp eax, 0x20
    je _v86_audio_init
    cmp eax, 0x21
    je _v86_audio_chime
    cmp eax, 0x22
    je _v86_audio_status
    cmp eax, 0x23
    je _v86_audio_shutdown
    cmp eax, 0x24
    je _v86_audio_sb_start
    cmp eax, 0x25
    je _v86_audio_set_vol
    cmp eax, 0x26
    je _v86_sb_io
    cmp eax, 0x27
    je _v86_sb_diag
    cmp eax, 0x28
    je _v86_sb_dma_diag
    cmp eax, 0x29
    je _v86_audio_irq11
    cmp eax, 0x2B
    je _v86_audio_external_prepare
    cmp eax, 0x2A
    je _v86_sb_mixer_ctl
    cmp eax, 0x36
    je _v86_ehci_recon
    cmp eax, 0x37
    je _v86_ehci_bringup
    cmp eax, 0x38
    je _v86_ehci_shutdown
    cmp eax, 0x39
    je _v86_m7101_info
    cmp eax, 0x3A
    je _v86_m7101_set_throttle
    cmp eax, 0x3B
    je _v86_m7101_set_backlight
    cmp eax, 0x3C
    je _v86_sd_init
    cmp eax, 0x3D
    je _v86_sd_read_sec
    cmp eax, 0x3E
    je _v86_sd_shutdown
    cmp eax, 0x40
    je _v86_sf2_begin
    cmp eax, 0x41
    je _v86_sf2_write_chunk
    cmp eax, 0x42
    je _v86_sf2_commit
    cmp eax, 0x43
    je _v86_sf2_unload
    cmp eax, 0x44
    je _v86_sf2_status

    or dword [ebp + CLIENT_EFLAGS], 1       ; unsupported -> set CF
    ret

.get_version:
    mov word [ebp + CLIENT_EAX], 0x0001     ; ABI 0.1
    mov word [ebp + CLIENT_EBX], GATE1_DEVICE_ID
    ret

.dispatch_v86:
    ; Convert V86 DS:SI and ES:DI to linear addresses below 1 MiB
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                            ; resp_ptr linear

    movzx ecx, word [ebp + CLIENT_DS]
    shl ecx, 4
    movzx edx, word [ebp + CLIENT_ESI]
    add ecx, edx                            ; req_ptr linear

    movzx edx, word [ebp + CLIENT_EDX]      ; resp_cap
    push edx
    push eax                                ; resp_ptr
    movzx edx, word [ebp + CLIENT_ECX]      ; req_len
    push edx
    push ecx                                ; req_ptr
    call gate1_svc_entry
    add esp, 16
    mov [ebp + CLIENT_EAX], eax
    ret

.irq11_call:
    mov eax, [ebp + CLIENT_EDX]             ; pit_tick
    push eax
    movzx eax, byte [ebp + CLIENT_ECX]      ; irq_line
    push eax
    mov eax, [ebp + CLIENT_EBX]             ; device_status
    push eax
    call gate1_irq11_isr
    add esp, 12
    mov [ebp + CLIENT_EAX], eax
    ret

; Packet-driver install support (V86 API AX=0x0003/5/6/7, driven by
; NETINIT.COM). Everything here is ring-0 work with no DOS calls: the
; DOS-facing half of the install (arena allocations, the INT 60h
; vector, staying resident) is NETINIT's own plain DOS code.

; AX=0x0003: probe the NIC and report install state + stub size.
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
    call gate1_net_stub_copy
    add esp, 4
    mov [ebp + CLIENT_EAX], eax
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
    push dword [net_dma_seg]
    call gate1_net_init
    add esp, 4
    cmp eax, 1
    je .nic_up
    cmp eax, 2
    jne .nic_fail
    mov word [ebp + CLIENT_EAX], 5 ; retain DMA until reboot
    ret
.nic_fail:
    mov word [ebp + CLIENT_EAX], 3
    ret
.nic_up:
    mov eax, PKT_BRIDGE_VECTOR
    mov esi, _pkt_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
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

; AX=0x0008: Gate 4b.3 ToPIC95 CardBus bridge bring-up.
; Returns AX = bridges found (0..2), BX = CardBus cards enumerated (0..2).
_v86_cb_init:
    call gate1_cb_init
    mov word [ebp + CLIENT_EAX], ax
    shr eax, 16
    mov word [ebp + CLIENT_EBX], ax
    ret

; AX=0x0009: copy 40-byte CardBusSlotInfo for slot BX (0/1) to ES:DI.
; Returns AX = 1 on success, 0 on invalid slot.
_v86_cb_slot_info:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    push eax                            ; dst_lin
    movzx eax, word [ebp + CLIENT_EBX]
    push eax                            ; slot
    call gate1_cb_slot_info
    add esp, 8
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x000A: Gate 4b.4a Atheros AR2413 MAC/EEPROM probe.
; Copies 44-byte WifiProbeInfo into ES:DI; returns AX = 1 if EEPROM OK, 0 otherwise.
_v86_wifi_probe:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    push eax                            ; dst_lin
    call gate1_wifi_probe
    add esp, 4
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x000B: Gate 4b.4b Atheros AR2413 2.4 GHz passive beacon scan.
; BX = caller-allocated 16 KiB DMA segment, ES:DI = 720-byte WifiScanResult buffer.
; Returns AX = ap_count (0..=16).
_v86_wifi_scan:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    push eax                            ; dst_lin
    movzx eax, word [ebp + CLIENT_EBX]
    push eax                            ; dma_seg
    call gate1_wifi_scan
    add esp, 8
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x000C: Gate 4b.5/4b.6 WPA2-PSK connect + INT 60h Wi-Fi backend activation.
; BX = caller-allocated 16 KiB DMA segment, CX = packet stub segment,
; ES:DI = 160-byte WifiConnectReq/Resp struct.
; Returns AX = 0 on success (and hooks INT 0F1h bridge if not already hooked).
_v86_wifi_connect:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; eax = req_lin
    movzx edx, word [ebp + CLIENT_EBX]  ; edx = dma_seg
    mov [wifi_saved_esp], esp
    mov esp, wifi_r0_stack_top
    push eax                            ; req_lin
    push edx                            ; dma_seg
    call gate1_wifi_connect
    mov esp, [wifi_saved_esp]
    mov word [ebp + CLIENT_EAX], ax
    or eax, eax
    jnz .wc_done
    movzx eax, word [ebp + CLIENT_ECX]
    mov [net_stub_seg], eax
    cmp dword [net_installed], 0
    jne .wc_done
    mov eax, PKT_BRIDGE_VECTOR
    mov esi, _pkt_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    mov dword [net_installed], 1
.wc_done:
    ret

; AX=0x000D: Non-destructive Wi-Fi diagnostic report (TX/RX counters, live
; AR2413 MAC/PHY register readback, 11g EEPROM calibration summary).
; ES:DI = output buffer, CX = capacity. Returns AX = bytes written.
_v86_wifi_diag:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    movzx edx, word [ebp + CLIENT_ECX]  ; cap
    push edx
    push eax
    call gate1_wifi_diag
    add esp, 8
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x000E: Switch INT 60h backend between Ethernet (BX=0) and Wi-Fi (BX=1).
; Returns AX = 1 on success, 0 if requested backend is not ready.
_v86_net_switch:
    movzx eax, word [ebp + CLIENT_EBX]
    push eax
    call gate1_net_switch_backend
    add esp, 4
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x000F: Cleanly disconnect Wi-Fi (Deauth + AR2413 reset + key wipe),
; power down CardBus 3.3 V slots with 250 ms discharge, and restore Ethernet
; on INT 60h if ready. Returns AX = 1.
_v86_wifi_disconnect:
    call gate1_wifi_disconnect
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0010: Initialize OHCI USB host controller, root hub, and external USB hubs.
; BX = 16 KiB DMA block segment, ES:DI = 416-byte UsbSummary buffer.
; Switches to the 8 KiB private Ring-0 stack for safe descriptor buffers.
; Returns AX = enumerated device count.
_v86_usb_init:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    movzx ecx, word [ebp + CLIENT_EBX]  ; dma_seg
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx                            ; save Jemm monitor ESP
    push eax                            ; dst_lin
    push ecx                            ; dma_seg
    call gate1_usb_init
    add esp, 8
    pop esp                             ; restore Jemm monitor ESP
    push eax                            ; preserve enumerated device count
    call _joy_sync_hooks                ; trap 0x201 + INT 15h AH=84h ONLY if USB gamepad present
    pop eax
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0011: Cleanly shut down OHCI lists/IRQs and unlock any active DMA block.
_v86_usb_shutdown:
    call _joy_remove_hooks
    call gate1_ehci_shutdown
    call gate1_usb_shutdown
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x001B: Get/set configurable usbdelay multiplier (1..=20) for slow USB flash drives.
; BX != 0 sets multiplier (clamped 1..=20); BX == 0 queries current multiplier.
; Returns active multiplier in AX.
_v86_usb_delay:
    movzx eax, word [ebp + CLIENT_EBX]
    or eax, eax
    jz .usb_delay_get
    push eax
    call gate1_usb_set_delay
    add esp, 4
    mov word [ebp + CLIENT_EAX], ax
    ret
.usb_delay_get:
    call gate1_usb_get_delay
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x001C: Read and clear accumulated USB HID mouse scroll wheel delta (dz).
; Returns signed 16-bit delta in AX.
_v86_usb_mouse_wheel:
    call gate1_usb_mouse_take_wheel
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0012: Format read-only USB Mass Storage root directory into ES:DI (CX=cap).
; Uses private 8 KiB Ring-0 stack for 512-byte sector buffers. Returns AX = bytes written.
_v86_usb_msc_dir:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    movzx ecx, word [ebp + CLIENT_ECX]  ; cap
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_usb_msc_dir
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0013: Open a file by NUL-terminated 8.3 path at ES:DI on read-only USB MSC.
; Returns AX = 1 if found, 0 otherwise.
_v86_usb_msc_open:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; name_lin
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    call gate1_usb_msc_open
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0014: Read next up-to-512-byte chunk of opened USB file into ES:DI.
; Returns AX = bytes read (0 = EOF, 0xFFFF = error).
_v86_usb_msc_read_chunk:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    call gate1_usb_msc_read_chunk
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0015: Fill 33-byte DOS BPB + part_lba + media_gen at ES:DI for USBDRV.SYS.
; Returns AX = 1 if FAT12/FAT16 USB volume ready (DX = media_gen lo), 0 otherwise.
_v86_usb_msc_bpb:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    call gate1_usb_msc_bpb
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    shr eax, 16
    mov word [ebp + CLIENT_EDX], ax
    ret

; AX=0x0016: Read one 512-byte sector at 32-bit LBA (DX:BX) from USB MSC
; into ES:DI. Returns AX = 1 on success, 0 on error.
_v86_usb_msc_read_sec:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx ecx, word [ebp + CLIENT_EDI]
    add eax, ecx                        ; dst_lin
    movzx ecx, word [ebp + CLIENT_EDX]
    shl ecx, 16
    movzx edx, word [ebp + CLIENT_EBX]
    or ecx, edx                         ; 32-bit LBA
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax                            ; dst_lin
    push ecx                            ; lba
    call gate1_usb_msc_read_sec
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0017: Gate 3 - Write one 512-byte sector at 32-bit LBA (DX:BX) to USB MSC
; from ES:DI. Returns AX = 1 on success, 0 on error.
_v86_usb_msc_write_sec:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx ecx, word [ebp + CLIENT_EDI]
    add eax, ecx                        ; src_lin
    movzx ecx, word [ebp + CLIENT_EDX]
    shl ecx, 16
    movzx edx, word [ebp + CLIENT_EBX]
    or ecx, edx                         ; 32-bit LBA
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax                            ; src_lin
    push ecx                            ; lba
    call gate1_usb_msc_write_sec
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0018: Gate 3 - Flush USB MSC write cache (SCSI SYNCHRONIZE CACHE(10)).
; Returns AX = 1 on success, 0 on error.
_v86_usb_msc_flush:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_usb_msc_flush
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0019: Gate 3 - Safe eject USB MSC (SYNCHRONIZE CACHE + START STOP UNIT).
; Returns AX = 1 on success, 0 if no USB drive mounted.
_v86_usb_msc_eject:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_usb_msc_eject
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x001A: Gate 3 - Probe live USB MSC media state via SCSI TEST UNIT READY.
; Returns AX = 0 (not ready), 1 (unchanged), 2 (changed); DX = media_gen.
_v86_usb_msc_media_check:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_usb_msc_media_check
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    shr eax, 16
    mov word [ebp + CLIENT_EDX], ax
    ret

; AX=0x0035: Stage F filesystem detection verb.
; Returns EAX = (media_gen << 16) | (flags << 8) | fs_type (0..5).
_v86_usb_fs_type:
    call gate1_usb_msc_fs_type
    mov [ebp + CLIENT_EAX], eax
    ret

; AX=0x0036: EHCI recon report (Stage H Step 2).
; ES:DI = report buffer, CX = capacity. Returns AX = report length.
; The Rust side may assign and map the EHCI BAR, so this runs on the
; wifi ring-0 stack like the other hardware-touching USB verbs.
_v86_ehci_recon:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    movzx ecx, word [ebp + CLIENT_ECX]  ; cap
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_ehci_recon
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0037: EHCI bring-up + status report (Stage H Step 3).
; BX = 16 KiB DMA block segment (same contract as AX=0x0010 USB init),
; ES:DI = report buffer, CX = capacity. Returns AX = report length.
_v86_ehci_bringup:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; dst_lin
    movzx ecx, word [ebp + CLIENT_ECX]  ; cap
    movzx ebx, word [ebp + CLIENT_EBX]  ; dma_seg
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    push ebx
    call gate1_ehci_bringup
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    shr eax, 16
    mov word [ebp + CLIENT_EBX], ax     ; BX=1 iff newly brought up (TSR must stay resident)
    ret

; AX=0x0038: Cleanly shut down EHCI, restore CONFIGFLAG=0 (companions own
; all ports), and unbind any EHCI MSC drive (Stage H Step 5 / EHCPROBE /U).
; Returns AX = 1 if EHCI was active and shut down, 0 if not active.
_v86_ehci_shutdown:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_ehci_shutdown
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0039: ALi M7101 PMU / Smart Battery / Throttle / LCD CCFT report.
; ES:DI = output buffer, CX = capacity. Returns AX = bytes written.
_v86_m7101_info:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_m7101_info
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x003A: Set ALi M7101 CPU clock throttle (BX = 0 off, 1..7 duty/8).
; ES:DI = output buffer, CX = capacity. Returns AX = bytes written.
_v86_m7101_set_throttle:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    movzx ebx, word [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    push ebx
    call gate1_m7101_set_throttle
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x003B: Set ALi M7101 LCD CCFT backlight (BX = 0 off, 1..7 PWM, 8 max).
; ES:DI = output buffer, CX = capacity. Returns AX = bytes written.
_v86_m7101_set_backlight:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    movzx ebx, word [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    push ebx
    call gate1_m7101_set_backlight
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x003C: Initialize Toshiba SD slot (1179:0805) and identify card (read-only).
; ES:DI = output buffer, CX = capacity. Returns AX = bytes written.
_v86_sd_init:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_sd_init
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x003D: Read one 512-byte sector from Toshiba SD card (read-only).
; EBX = LBA, ES:DI = 512-byte buffer. Returns AX = 0 on success, non-zero error.
_v86_sd_read_sec:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov ebx, [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    push ebx
    call gate1_sd_read_sec
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x003E: Cleanly power down Toshiba SD slot and stop clocks.
_v86_sd_shutdown:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_sd_shutdown
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0040: Begin loading an .SF2 bank into locked XMS physical memory.
; EBX = phys_base, ECX = file_size, DX = xms_handle.
; Returns AX = 0 ok / non-zero error, DX = previous XMS handle to free (if any).
_v86_sf2_begin:
    mov ebx, [ebp + CLIENT_EBX]
    mov ecx, [ebp + CLIENT_ECX]
    movzx eax, word [ebp + CLIENT_EDX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    push ecx
    push ebx
    call gate1_sf2_begin
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    shr eax, 16
    mov word [ebp + CLIENT_EDX], ax
    ret

; AX=0x0041: Copy CX bytes from ES:DI into XMS SF2 buffer at byte offset EBX.
; Returns AX = 0 on success.
_v86_sf2_write_chunk:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; src_lin
    mov ebx, [ebp + CLIENT_EBX]         ; offset
    movzx ecx, word [ebp + CLIENT_ECX]  ; len
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    push ebx
    call gate1_sf2_write_chunk
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0042: Parse uploaded .SF2 bank in XMS and activate it.
; ES:DI = status/error output buffer, CX = capacity.
; Returns AX = report length, BX = 0 on success / 1 on parse error.
_v86_sf2_commit:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_sf2_commit
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    shr eax, 16
    mov word [ebp + CLIENT_EBX], ax
    ret

; AX=0x0043: Unload active .SF2 bank (reverts to built-in ROM).
; Returns AX = previous XMS handle (0 if none).
_v86_sf2_unload:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_sf2_unload
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0044: Print current .SF2 SoundFont status into ES:DI (cap CX).
; Returns AX = bytes written.
_v86_sf2_status:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    movzx ecx, word [ebp + CLIENT_ECX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ecx
    push eax
    call gate1_sf2_status
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

_v86_audio_external_prepare:
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_audio_external_prepare
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0020: Initialize ALi M5451 AC'97 audio controller.
; BX = 16 KiB DMA segment (or 0 to reuse), CX = flags (bit 0 = invert EAPD),
; ES:DI = 40-byte AudioInfo output buffer. Returns AX = 0 on success.
_v86_audio_init:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; out_ptr
    movzx ecx, word [ebp + CLIENT_ECX]  ; flags
    movzx ebx, word [ebp + CLIENT_EBX]  ; dma_seg
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax                            ; out_ptr
    push ecx                            ; flags
    push ebx                            ; dma_seg
    call gate1_audio_init
    add esp, 12
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0021: Play 3-note chime (C5-E5-G5) via ALi M5451 Voice 0.
; CX = flags (bit 0 = invert EAPD), ES:DI = 40-byte AudioInfo output buffer.
_v86_audio_chime:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; out_ptr
    movzx ecx, word [ebp + CLIENT_ECX]  ; flags
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax                            ; out_ptr
    push ecx                            ; flags
    call gate1_audio_chime
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0022: Read ALi M5451 audio status into ES:DI (40-byte AudioInfo).
_v86_audio_status:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; out_ptr
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax                            ; out_ptr
    call gate1_audio_status
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0023: Stop ALi M5451 voices, remove SB/AdLib I/O traps, and mute cleanly.
_v86_audio_shutdown:
    call _sb_remove_traps
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    call gate1_audio_shutdown
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0024: Start resident Sound Blaster Pro (0x220..0x22F) + AdLib OPL2
; (0x388..0x389) streaming on ALi M5451 Voice 0 (22,050 Hz S16 stereo).
; BX = 16 KiB DMA segment, ES:DI = 40-byte AudioInfo output buffer.
; Returns AX = 0 on success.
_v86_audio_sb_start:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx                        ; out_ptr
    movzx ebx, word [ebp + CLIENT_EBX]  ; dma_seg
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax                            ; out_ptr
    push ebx                            ; dma_seg
    call gate1_audio_sb_start
    add esp, 8
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    or eax, eax
    jnz .sb_start_done
    call _sb_install_traps
.sb_start_done:
    ret

; AX=0x0025: Set live AC'97 volume (BX = 0..9, 0=mute, 9=max). Returns AX = 0 on ok.
_v86_audio_set_vol:
    movzx ebx, word [ebp + CLIENT_EBX]
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push ebx
    call gate1_audio_set_vol
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0026: Sound Blaster / OPL2 I/O bridge (called from HDPMI32i PM32 trap via
; DPMI INT 31h AX=0301h). DX=port, CX=out (0=IN, 1=OUT), BX=data byte.
; Returns AL=read byte (for IN). Sets AH=1 ONLY when port 0x22C (DSP command,
; e.g. 0xF2 IRQ probe) triggered an immediate IRQ; continuous DMA IRQs are
; delivered cleanly at PM32 INT 1Ch boundaries via BDA 0040:00F4 so OPL2 writes
; are never re-entered.
_v86_sb_io:
    movzx eax, word [ebp + CLIENT_ECX]
    or eax, eax
    jz .v86_sb_in
    mov eax, 4                          ; IO_FLAG_OUTPUT (bit 2)
.v86_sb_in:
    movzx ecx, word [ebp + CLIENT_EBX]  ; val
    movzx edx, word [ebp + CLIENT_EDX]  ; port
    push esi                            ; preserve ESI for JEMM V86 dispatcher
    push ecx                            ; save val
    push edx                            ; save port for 0x22C check
    mov esi, esp
    mov esp, audio_r0_stack_top
    push esi
    push ecx
    push eax
    push edx
    call gate1_sb_io
    add esp, 12
    pop esp
    pop edx
    pop ecx
    pop esi
    and eax, 0xFF
    cmp edx, 0x22C
    jne .v86_sb_no_irq
    cmp word [ebp + CLIENT_ECX], 0      ; must be OUT (never steal on IN 0x22C!)
    je .v86_sb_no_irq
    cmp cl, 0xF2                        ; ONLY DSP 0xF2 immediate IRQ probe
    jne .v86_sb_no_irq
    push eax
    call gate1_sb_take_one_irq
    or eax, eax
    pop eax
    jz .v86_sb_no_irq
    or eax, 0x0100                      ; immediate PM32 INT 0Dh for DSP 0xF2
.v86_sb_no_irq:
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0027: Copy 32-byte SbDiag telemetry snapshot into ES:DI.
_v86_sb_diag:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    call gate1_sb_diag
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0028: Copy 16-byte SbDmaDiag telemetry snapshot into ES:DI.
_v86_sb_dma_diag:
    movzx eax, word [ebp + CLIENT_ES]
    shl eax, 4
    movzx edx, word [ebp + CLIENT_EDI]
    add eax, edx
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    call gate1_sb_dma_diag
    add esp, 4
    pop esp
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x0029: Service shared PCI IRQ 11 (called from AUDPROBE's real-mode
; IVT 0x73 ISR when HDPMI32i reflects hardware IRQ 11 from PM32 to real mode).
; Checks ALi M5451 first, then active Wi-Fi/NIC (gate1_net_service) so an
; AR2413 RXOK IRQ during PM32 (e.g. DOOM) never falls through to the BIOS
; default IVT 0x73 stub (which masks IRQ 11 on slave PIC 0xA1 and kills audio!).
; Returns AL=1 if M5451 or NIC owned the interrupt (and PIC EOI was sent),
; AH=pending SB IRQ count (0..4) to deliver via real-mode INT 0Dh.
_v86_audio_irq11:
    mov edx, esp
    mov esp, audio_r0_stack_top
    push edx
    call gate1_audio_irq11
    pop esp
    test eax, 1
    jz .v86_a11_try_net
    mov al, 0x20
    out 0xA0, al
    out 0x20, al
    push dword 4
    call gate1_sb_take_up_to            ; take at most 4 pending SB IRQs; leave any remainder queued
    add esp, 4
    mov ah, al                          ; AH = pending SB IRQ count (0..4)
    mov al, 1                           ; AL = 1 (M5451 owned IRQ 11)
    jmp .v86_a11_done
.v86_a11_try_net:
    call gate1_net_service
    test eax, 1
    jz .v86_a11_done
    mov al, 0x20
    out 0xA0, al
    out 0x20, al
    mov ax, 0x0001                      ; AL = 1 (NIC/Wi-Fi owned IRQ 11), AH = 0 (no SB IRQ)
.v86_a11_done:
    mov word [ebp + CLIENT_EAX], ax
    ret

; AX=0x002A: Get/set AC'97 + SB Pro mixer levels (BX=cmd, CX=val).
; Returns packed dword in EAX (AL=ac97, AH=master, high word=voice|fm<<8).
_v86_sb_mixer_ctl:
    movzx eax, word [ebp + CLIENT_ECX]  ; val
    movzx ecx, word [ebp + CLIENT_EBX]  ; cmd
    mov edx, esp
    mov esp, wifi_r0_stack_top
    push edx
    push eax
    push ecx
    call gate1_sb_mixer_ctl
    add esp, 8
    pop esp
    mov [ebp + CLIENT_EAX], eax
    ret

; Ring-0 JEMM386 I/O trap callback for Sound Blaster Pro (0x220..0x22F) and
; AdLib OPL2 (0x388..0x389).
; Entry (JLM.INC / QPIEMU.ASM): EDX=port, ECX=io_type, EAX=data, EBP=client.
; Exit: EAX=read value (for IN), EBX/ESI/EDI/EBP preserved.
_sb_io_handler:
    push ebx
    push esi
    push edi
    push eax                            ; val
    push ecx                            ; io_type
    push edx                            ; port
    call gate1_sb_io
    add esp, 12
    ; If a V86 program triggered an immediate SB IRQ (e.g. DSP 0xF2 in ST3)
    ; and hooked real-mode IVT 0x0D (IRQ 5), deliver INT 0Dh immediately.
    push eax
    cmp dword [sb_in_irq], 0
    jne .sb_io_no_irq
    mov eax, [0x00000034]
    or eax, eax
    jz .sb_io_no_irq
    mov edx, eax
    shr edx, 16
    shl edx, 4
    and eax, 0xFFFF
    add edx, eax
    cmp byte [edx], 0xCF
    je .sb_io_no_irq
    call gate1_sb_take_one_irq      ; consume ONE event for the one INT 0Dh
                                    ; delivered here; any others stay pending
                                    ; for _sb_deliver_pending_irq's loop
    or eax, eax
    jz .sb_io_no_irq
    mov dword [sb_in_irq], 1
    mov esi, ebp
    mov edi, sb_saved_client
    mov ecx, 19
    rep movsd
    VMMCall VMM_BEGIN_NEST_EXEC
    mov eax, SB_IRQ5_VECTOR
    VMMCall VMM_EXEC_INT
    VMMCall VMM_END_NEST_EXEC
    mov esi, sb_saved_client
    mov edi, ebp
    mov ecx, 19
    rep movsd
    mov dword [sb_in_irq], 0
.sb_io_no_irq:
    pop eax
    pop edi
    pop esi
    pop ebx
    ret

_sb_install_traps:
    cmp dword [sb_traps_installed], 0
    jne .sb_inst_done
    push ebx
    push esi
    mov ebx, 0x220
.sb_inst_220:
    mov edx, ebx
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    inc ebx
    cmp ebx, 0x230
    jb .sb_inst_220
    mov edx, 0x388
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov edx, 0x389
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    ; OPL3 Bank 1 (0x38A/0x38B) + MPU-401 UART General MIDI (0x330/0x331)
    mov edx, 0x38A
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov edx, 0x38B
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov edx, 0x330
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov edx, 0x331
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    ; Virtualize ONLY Channel-1-dedicated ISA DMA ports in V86 mode (0x02, 0x03, 0x83).
    ; Leave shared 8237 ports 0x0A/0x0B/0x0C with JEMM386's VDMA handler so Floppy
    ; Channel 2 (0x04/0x05) flip-flop & UMB bounce translation are never bypassed.
    mov edx, 0x02
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov edx, 0x03
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov edx, 0x83
    mov esi, _sb_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov dword [sb_traps_installed], 1
    pop esi
    pop ebx
.sb_inst_done:
    ret

_sb_remove_traps:
    cmp dword [sb_traps_installed], 0
    je .sb_rem_done
    push ebx
    mov ebx, 0x220
.sb_rem_220:
    mov edx, ebx
    VMMCall VMM_REMOVE_IO_HANDLER
    inc ebx
    cmp ebx, 0x230
    jb .sb_rem_220
    mov edx, 0x388
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x389
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x38A
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x38B
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x330
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x331
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x02
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x03
    VMMCall VMM_REMOVE_IO_HANDLER
    mov edx, 0x83
    VMMCall VMM_REMOVE_IO_HANDLER
    mov dword [sb_traps_installed], 0
    pop ebx
.sb_rem_done:
    ret

; Ring-0 JEMM386 I/O trap callback for virtual PC Gameport (port 0x201).
; Installed ONLY when a USB HID Gamepad is detected so real gameports are untouched.
; Entry: EDX=port, ECX=io_type (bit 2 = IO_FLAG_OUTPUT), EAX=data, EBP=client.
; Exit: EAX=read value (for IN), EBX/ESI/EDI/EBP preserved.
_joy_io_handler:
    push ebx
    push esi
    push edi
    mov ebx, eax                        ; preserve client upper EAX bits for JEMM writeback
    shr ecx, 2                          ; bit 2 (IO_FLAG_OUTPUT) -> bit 0
    and ecx, 1                          ; is_write: 1 = OUT, 0 = IN
    push ecx                            ; is_write
    push eax                            ; val
    push edx                            ; port
    call gate1_joy_io
    add esp, 12
    and eax, 0x000000FF
    and ebx, 0xFFFFFF00
    or eax, ebx
    pop edi
    pop esi
    pop ebx
    ret

; BIOS INT 15h AH=84h (Joystick Support) chain hook, installed ONLY when a USB
; HID Gamepad is detected. Non-84h calls chain immediately with zero overhead.
    dd _int15_oldvec
_int15_hookproc:
    cmp byte [ebp + CLIENT_EAX + 1], 0x84
    jne .int15_chain
    pushad
    push ebp
    call gate1_joy_int15
    add esp, 4
    or eax, eax
    jz .int15_pop_chain
    popad
    clc                                 ; handled by USB gamepad driver
    ret
.int15_pop_chain:
    popad
.int15_chain:
    stc                                 ; chain to BIOS INT 15h
    ret

_joy_install_hooks:
    cmp dword [joy_hooks_installed], 0
    jne .joy_inst_done
    push esi
    mov edx, JOY_GAMEPORT
    mov esi, _joy_io_handler
    VMMCall VMM_INSTALL_IO_HANDLER
    mov eax, INT15_VECTOR
    mov esi, _int15_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    mov dword [joy_hooks_installed], 1
    pop esi
.joy_inst_done:
    ret

_joy_remove_hooks:
    cmp dword [joy_hooks_installed], 0
    je .joy_rem_done
    push esi
    mov edx, JOY_GAMEPORT
    VMMCall VMM_REMOVE_IO_HANDLER
    mov eax, INT15_VECTOR
    mov esi, _int15_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    mov dword [joy_hooks_installed], 0
    pop esi
.joy_rem_done:
    ret

_joy_sync_hooks:
    call gate1_joy_present
    or eax, eax
    jz _joy_remove_hooks
    jmp _joy_install_hooks

; C-ABI helper called from Rust (cardbus.rs / future MMIO drivers):
;   u32 jlm_map_mmio(u32 phys, u32 npages)
; Reserves `npages` in PR_SYSTEM and commits them uncached to `phys`.
; Returns mapped flat linear address (4 KiB-aligned) or 0 on failure.
global jlm_map_mmio
jlm_map_mmio:
    push ebx
    push esi
    mov ebx, [esp + 12]                 ; phys
    mov esi, [esp + 16]                 ; npages
    or esi, esi
    jz .mmio_fail
    push dword 0                        ; flags = 0
    push esi                            ; npages
    push dword PR_SYSTEM
    VMMCall VMM_PAGE_RESERVE
    add esp, 12
    cmp eax, 0xFFFFFFFF
    je .mmio_fail
    or eax, eax
    jz .mmio_fail
    push eax                            ; save linear base
    mov ecx, eax
    shr ecx, 12                         ; lin_page
    mov edx, ebx
    shr edx, 12                         ; phys_page
    push dword PC_MMIO_FLAGS
    push edx                            ; phys_page
    push esi                            ; npages
    push ecx                            ; lin_page
    VMMCall VMM_PAGE_COMMIT_PHYS
    add esp, 16
    or eax, eax
    pop eax                             ; restore linear base (flags preserved)
    jz .mmio_fail
    pop esi
    pop ebx
    ret
.mmio_fail:
    xor eax, eax
    pop esi
    pop ebx
    ret

; Protected-mode (DPMI / HDPMI32i) API dispatcher.
; Client registers in [ebp]:
;   AX = 0x0000: Get JLM/ABI version -> AX = 0x0001, BX = 0xFD01
;   AX = 0x0001: Dispatch ABI request with flat 32-bit ESI = req_ptr, ECX = req_len,
;                EDI = resp_ptr, EDX = resp_cap -> Client_EAX = StatusCode
_pm_dispatch:
    VMMCall VMM_SIMULATE_FAR_RET
    and dword [ebp + CLIENT_EFLAGS], ~1

    movzx eax, word [ebp + CLIENT_EAX]
    cmp eax, 0
    je .pm_version
    cmp eax, 1
    je .pm_call
    cmp eax, 0x26
    je .pm_sb_io

    or dword [ebp + CLIENT_EFLAGS], 1
    ret

; PM32 AX=0x0026: Sound Blaster / OPL2 I/O trap bridge from HDPMI32i PM32 clients
; (e.g. DOOM / DOS4GW). EDX = port, ECX = out (0=IN, 1=OUT), EBX = data byte.
; Returns EAX = read byte (for IN).
.pm_sb_io:
    mov eax, [ebp + CLIENT_ECX]
    or eax, eax
    jz .pm_sb_in
    mov eax, 4                          ; IO_FLAG_OUTPUT (bit 2)
.pm_sb_in:
    push dword [ebp + CLIENT_EBX]       ; val
    push eax                            ; io_type
    push dword [ebp + CLIENT_EDX]       ; port
    call gate1_sb_io
    add esp, 12
    mov [ebp + CLIENT_EAX], eax
    ret

.pm_version:
    mov word [ebp + CLIENT_EAX], 0x0001
    mov word [ebp + CLIENT_EBX], GATE1_DEVICE_ID
    ret

.pm_call:
    push dword [ebp + CLIENT_EDX]           ; resp_cap
    push dword [ebp + CLIENT_EDI]           ; resp_ptr
    push dword [ebp + CLIENT_ECX]           ; req_len
    push dword [ebp + CLIENT_ESI]           ; req_ptr
    call gate1_svc_entry
    add esp, 16
    mov [ebp + CLIENT_EAX], eax
    ret

; Shared IRQ 11 chain hook, installed on V86 vector 0x73 at attach.
; Jemm calls this in Ring 0 with IF=0 for every V86 INT 73h, hardware
; or software, before the real-mode IVT handler runs. Since Gate 2
; step 2.3 the hook also services the 82550 (device-level acknowledge
; and receiver upcalls, in the proc body below), but its disposition
; is unchanged: it always returns CF set, which hands the interrupt
; to the next hook or to the IVT handler untouched - that handler,
; never us, owns PIC EOI. Claiming or EOI-ing here would starve the
; chained handlers behind us.
    dd _irq11_oldvec            ; hook procs are preceded by the address of their old-vector storage (JLM.INC HookProc)
_irq11_hookproc:
    pushad
    ; Gate 4a: ALi M5451 AC'97 half-buffer audio interrupt (86.1 Hz / 11.61 ms).
    ; Checked first on shared level-triggered PCI IRQ 11.
    mov edx, esp
    mov esp, audio_r0_stack_top
    push edx
    call gate1_audio_irq11
    pop esp
    test eax, 1
    jz .irq11_try_net
    ; M5451 owned and acknowledged this IRQ 11. Send EOI and return cleanly.
    ; Do NOT call VMM_EXEC_INT(0x0D) from inside hardware INT 73h (HDPMI32i
    ; drops nested PM32 reflections from hardware IRQ 11). Instead leave
    ; irq_pending_count queued so hdpmi_sb_trap delivers INT 0Dh natively in
    ; PM32 on the very next OPL2/SB port access (~1-7 ms at 140 Hz in DOOM),
    ; with _timer_hookproc (INT 1Ch) as a safety net during idle probes.
    mov al, 0x20
    out 0xA0, al
    out 0x20, al
    popad
    clc
    ret
.irq11_try_net:
    ; Gate 2 step 2.3: the 82550 lives on this line now. Service it
    ; first (the Rust front half reads and acknowledges the device
    ; status, observing the shared-IRQ discipline), then run the
    ; receiver upcalls it armed. The upcall nest saves and restores
    ; the whole client block around the receiver call, exactly as the
    ; INT 33h callback dispatch does: nested execution returns the
    ; receiver's registers in the block, and the interrupted program
    ; must never see them.
    call gate1_net_service
    push eax                            ; save claimed flag (1 = 82550 owned IRQ 11, 0 = chain)
    call _net_drain_upcalls
    pop eax                             ; claimed flag from gate1_net_service
    or eax, eax
    jz .irq11_chain
    ; The 82550 owned and acknowledged this IRQ 11. Send non-specific EOI to
    ; slave PIC (0xA0) then master PIC (0x20) and claim (CF=0) so a BIOS
    ; default INT 73h handler cannot re-mask IRQ 11 on real hardware. Because
    ; PCI IRQ 11 is level-triggered (ELCR 0x4D1 bit 3 = 1), any other device
    ; asserting simultaneously immediately re-raises IRQ 11 after EOI and takes
    ; the chain path below.
    mov al, 0x20
    out 0xA0, al
    out 0x20, al
    popad
    clc
    ret
.irq11_chain:
    mov eax, [0x46C]            ; BIOS tick count at 0040:006C, identity-mapped low memory
    push eax                    ; pit_tick
    push 11                     ; irq_line
    push 0                      ; device_status for the Gate 1 telemetry call
    call gate1_irq11_isr
    add esp, 12
    popad
    stc                         ; chain onward when not owned by 82550
    ret

; Shared upcall drain helper (called by _irq11_hookproc and _timer_hookproc).
; Expects EBP = Client_Reg_Struc pointer; preserves EBP.
; Guarded against re-entrancy because VMM_RESUME_EXEC can run the client
; receiver with IF=1 (or the receiver may execute STI); a nested INT 1Ch or
; IRQ 11 must never clobber net_saved_client, net_upcall_buf, or wifi_rx_buf.
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

; PIT timer chain hook (INT 08h, 18.2 Hz): when Wi-Fi is active, polls the
; AR2413 RX ring and delivers ready frames via INT 60h upcalls so CardBus
; Wi-Fi works regardless of BIOS PCI PIR routing. Always chains (CF=1).
    dd _timer_oldvec
_timer_hookproc:
    pushad
    call gate1_usb_poll
    ; Deliver any pending INT 33h AX=000Ch mouse callback asynchronously on the
    ; 18.2 Hz timer tick so callback-driven apps (Impulse Tracker, TTDX, etc.)
    ; that never poll INT 33h in their main loop still get live mouse movement.
    call _mouse_deliver_cb
    ; Refill ALi M5451 ring buffer in block-synchronized chunks: whenever an SB
    ; DMA block completes (eax == 1 or 2), deliver INT 0Dh immediately so the
    ; game mixes the NEXT block into its DMA buffer before we render it!
    mov ecx, 8
.timer_audio_loop:
    push ecx
    mov edx, esp
    mov esp, audio_r0_stack_top
    push edx
    call gate1_audio_poll
    pop esp
    pop ecx
    cmp eax, 0
    je .timer_no_sb_irq
    cmp eax, 1
    je .timer_audio_irq_done
    cmp eax, 2
    je .timer_audio_irq_more
    ; eax == 3: more frames, no IRQ
    dec ecx
    jnz .timer_audio_loop
    jmp .timer_no_sb_irq
.timer_audio_irq_more:
    push ecx
    call _sb_deliver_pending_irq
    pop ecx
    dec ecx
    jnz .timer_audio_loop
    jmp .timer_no_sb_irq
.timer_audio_irq_done:
    call _sb_deliver_pending_irq
.timer_no_sb_irq:
    call gate1_net_poll_wifi
    or eax, eax
    jz .timer_chain
    call _net_drain_upcalls
.timer_chain:
    popad
    stc
    ret

; Helper: drain pending SB IRQ 5 count and either fire V86 INT 0Dh (if a
; real-mode game hooked IVT 0x0D) or add to the BDA 0040:00F4 counter for
; HDPMI32i's PM32 INT 08h hook (DOOM/DOS4GW). Preserves EBP.
_sb_deliver_pending_irq:
    call gate1_sb_take_irq
    or eax, eax
    jz .sb_del_done
    mov ecx, eax                        ; pending IRQ count (1..N)
    mov eax, [0x00000034]               ; IVT vector 0x0D (IRQ 5)
    or eax, eax
    jz .sb_del_pm
    mov edx, eax
    shr edx, 16
    shl edx, 4
    and eax, 0xFFFF
    add edx, eax
    cmp byte [edx], 0xCF                ; default IRET stub -> PM32 client
    je .sb_del_pm
    ; Real-mode (or HDPMI32i-reflected) IVT 0x0D: deliver ALL `ecx` pending IRQs
    ; so multi-block ticks (e.g. DOOM's 11.5 ms stereo blocks at 18.2 Hz) stay 1-to-1!
    cmp ecx, 8
    jbe .sb_v86_loop
    mov ecx, 8
.sb_v86_loop:
    push ecx
    push esi
    push edi
    mov esi, ebp
    mov edi, sb_deliv_saved_client
    mov ecx, 19
    rep movsd
    pop edi
    pop esi
    VMMCall VMM_BEGIN_NEST_EXEC
    mov eax, SB_IRQ5_VECTOR
    VMMCall VMM_EXEC_INT
    VMMCall VMM_END_NEST_EXEC
    push esi
    push edi
    mov esi, sb_deliv_saved_client
    mov edi, ebp
    mov ecx, 19
    rep movsd
    pop edi
    pop esi
    pop ecx
    dec ecx
    jnz .sb_v86_loop
    ret
.sb_del_pm:
    ; Protected-mode game: accumulate count into BDA 0040:00F4 (capped at 8)
    movzx eax, byte [0x000004F4]
    add eax, ecx
    cmp eax, 8
    jbe .sb_del_store
    mov eax, 8
.sb_del_store:
    mov [0x000004F4], al
.sb_del_done:
    ret

; Packet-driver bridge hook on INT 0F1h. The real-mode stub at the
; INT 60h vector forwards every packet call here with the client's
; registers untouched; gate1_net_api implements the specification
; against the client block and writes the results (including the
; error carry in the block's EFLAGS) back into it. The hook claims:
; nothing else lives on 0F1h to chain to.
    dd _pkt_oldvec
_pkt_hookproc:
    pushad
    push ebp
    call gate1_net_api
    add esp, 4
    popad
    clc
    ret

; PS/2 packet hook on V86 vector 0x74 (IRQ 12), installed at attach
; only when gate1_mouse_init found a mouse. The 8042 status port is
; the arbiter: bit 0 says a byte waits, bit 5 says it belongs to the
; auxiliary device. Anything else chains untouched (keyboard data is
; not ours to consume). Our byte is read here and fed to the Rust
; packet assembler; the hook then chains onward so the monitor's EOI
; path runs - claiming starved the PIC after the first byte (see the
; note at the claim site below).
    dd _mouse_irq_oldvec
_mouse_irq_hookproc:
    pushad
    mov ecx, 12                 ; drain up to 4 complete 3-byte aux packets per IRQ 12
.drain_lp:
    in al, 0x64
    and al, 0x21
    cmp al, 0x21
    jne .eoi_claim              ; nothing waiting, or keyboard data (not aux)
    in al, 0x60
    movzx eax, al
    push ecx
    push eax
    call gate1_mouse_feed
    add esp, 4
    pop ecx
    dec ecx
    jnz .drain_lp
.eoi_claim:
    ; Send non-specific EOI to slave PIC (IRQ 12 = slave IRQ 4) and master PIC,
    ; then claim private INT 0xF3 bridge (CF=0) so the real-mode stub executes IRET.
    mov al, 0x20
    out 0xA0, al
    out 0x20, al
    popad
    clc
    ret

; Helper: if a mouse INT 33h AX=000Ch callback is registered and events are
; pending, invoke it via JEMM nested execution with (AX=event mask, BX=buttons,
; CX/DX=position, SI/DI=mickeys). Guarded by mouse_in_cb against re-entrancy.
_mouse_deliver_cb:
    cmp dword [mouse_in_cb], 0
    jne .mcb_done
    push mouse_cb_info_buf
    call gate1_mouse_cb_info
    add esp, 4
    or eax, eax
    jz .mcb_done
    mov dword [mouse_in_cb], 1
    push esi
    push edi
    mov esi, ebp
    mov edi, mouse_saved_client
    mov ecx, 19
    rep movsd
    pop edi
    pop esi
    VMMCall VMM_BEGIN_NEST_EXEC
    mov eax, [mouse_cb_info_buf + 4]
    mov [ebp + CLIENT_EAX], eax
    mov eax, [mouse_cb_info_buf + 8]
    mov [ebp + CLIENT_EBX], eax
    mov eax, [mouse_cb_info_buf + 12]
    mov [ebp + CLIENT_ECX], eax
    mov eax, [mouse_cb_info_buf + 16]
    mov [ebp + CLIENT_EDX], eax
    mov eax, [mouse_cb_info_buf + 20]
    mov [ebp + CLIENT_ESI], eax
    mov eax, [mouse_cb_info_buf + 24]
    mov [ebp + CLIENT_EDI], eax
    mov ecx, [mouse_cb_info_buf]
    mov edx, ecx
    and edx, 0xFFFF                     ; offset
    shr ecx, 16                         ; segment
    VMMCall VMM_SIMULATE_FAR_CALL
    VMMCall VMM_RESUME_EXEC
    VMMCall VMM_END_NEST_EXEC
    push esi
    push edi
    mov esi, mouse_saved_client
    mov edi, ebp
    mov ecx, 19
    rep movsd
    pop edi
    pop esi
    mov dword [mouse_in_cb], 0
.mcb_done:
    ret

; Bridge hook on V86 vector 0xF2 for 32-bit DPMI clients (DOOM / Dillo / DJGPP /
; DOS4GW under HDPMI32i) that invoke INT 33h via DPMI 0300h: HDPMI32i jumps
; directly to IVT[0x33] (0040:00F0), bypassing JEMM's V86 INT 33h chain hook.
; Does NOT call _mouse_deliver_cb inside 0300h to avoid nesting; _timer_hookproc
; delivers 000Ch callbacks on INT 1Ch.
    dd _mouse_bridge_oldvec
_mouse_bridge_hookproc:
    pushad
    push ebp
    call gate1_mouse_api
    add esp, 4
    popad
    clc                                 ; always claim private INT 0xF2 bridge
    ret

; DOS mouse API hook on V86 vector 0x33. Two jobs per entry, in order:
; (1) if packet events are pending for a registered callback, fire it via
; _mouse_deliver_cb; (2) dispatch the API call itself to the Rust core.
    dd _int33_oldvec
_int33_hookproc:
    pushad
    call _mouse_deliver_cb
    push ebp
    call gate1_mouse_api
    add esp, 4
    or eax, eax
    jz .chain
    popad
    clc                                 ; handled by the resident driver
    ret
.chain:
    popad
    stc
    ret

; JLOAD PE32 entry point: BOOL __stdcall DllMain(hModule, dwReason, pJLComm)
global DllMain
DllMain:
    mov eax, [esp + 8]                      ; dwReason
    cmp eax, DLL_PROCESS_ATTACH
    jne .check_detach
    ; Temporary whole-module exclusion until the shared claim service exists.
    push edi
    xor edi, edi
    mov eax, 0xFD05
    VMMCall VMM_GET_DDB
    test ecx, ecx
    jnz .attach_conflict
    xor edi, edi
    mov eax, 0xFD06
    VMMCall VMM_GET_DDB
    test ecx, ecx
    jnz .attach_conflict
    pop edi
    call gate1_svc_reset
    ; This PE is only ever loaded by JLOAD, so Jemm's VDMAD services are
    ; guaranteed present here; the standalone harness never runs DllMain.
    push 1
    call gate1_svc_set_platform_vdmad
    add esp, 4
    ; Observe the shared IRQ 11 chain. A failed hook is not fatal: the
    ; module still serves API calls exactly as before, only the IRQ
    ; telemetry stays dark.
    mov eax, IRQ11_VECTOR
    mov esi, _irq11_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    mov eax, TIMER_IRQ_VECTOR
    mov esi, _timer_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    ; Gate 2 mouse: bring up the PS/2 device while the data port is
    ; still ours to poll (the response bytes are consumed by the init
    ; itself, with IRQ 12 held off at the controller until it is
    ; done), then hook what applies. The INT 33h hook is installed
    ; EITHER WAY: with no mouse it answers function 0000h with "no
    ; mouse" and keeps the Ferric diagnostics (0021h-0023h) reachable,
    ; so a failed bring-up on real hardware reports its stage instead
    ; of looking identical to an absent driver. The packet IRQ hook
    ; is installed only when a device actually answered.
    call gate1_mouse_init
    push eax
    mov eax, INT33_VECTOR
    mov esi, _int33_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    mov eax, MOUSE_BRIDGE_VECTOR
    mov esi, _mouse_bridge_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    mov dword [mouse_hooks_installed], 1
    ; Populate real-mode IVT INT 33h (0000:00CC) with a 3-byte bridge stub
    ; (`INT 0xF2; IRET` = CD F2 CF at BDA IACA 0040:00F0..00F2) if currently zero
    ; OR pointing at a dummy IRET (0xCF) stub installed by FreeDOS KERNEL.SYS.
    ; Satisfies both real-mode getvect(0x33) checks and PM32 DPMI 0300h calls.
    mov eax, [0x000000CC]
    or eax, eax
    jz .do_patch_int33
    mov edx, eax
    shr edx, 16
    shl edx, 4
    and eax, 0xFFFF
    add edx, eax
    cmp byte [edx], 0xCF                ; dummy IRET stub?
    jne .int33_ivt_kept
.do_patch_int33:
    mov eax, [0x000000CC]
    mov [mouse_old_ivt33], eax
    mov word [0x000004F0], 0xF2CD       ; INT 0xF2 (CD F2) at 0040:00F0
    mov byte [0x000004F2], 0xCF         ; IRET (CF) at 0040:00F2
    mov dword [0x000000CC], 0x004000F0  ; 0040:00F0
    mov dword [mouse_ivt_patched], 1
.int33_ivt_kept:
    pop eax
    or eax, eax
    jz .attach_done
    ; Route IVT[0x74] (IRQ 12) through a 3-byte real-mode stub at BDA IACA
    ; 0040:00F5 (`INT 0xF3; IRET` = CD F3 CF) so BOTH V86 IRQ 12 AND PM32
    ; HDPMI32i-reflected IRQ 12 (which jumps directly to IVT[0x74], bypassing
    ; JEMM's V86 INT 74h chain hook!) always enter _mouse_irq_hookproc.
    mov eax, MOUSE_IRQ_BRIDGE_VECTOR
    mov esi, _mouse_irq_hookproc
    VMMCall VMM_HOOK_V86_INT_CHAIN
    mov eax, [0x000001D0]               ; save original IVT[0x74]
    mov [mouse_old_ivt74], eax
    mov word [0x000004F5], 0xF3CD       ; INT 0xF3 (CD F3) at 0040:00F5
    mov byte [0x000004F7], 0xCF         ; IRET (CF) at 0040:00F7
    mov dword [0x000001D0], 0x004000F5  ; IVT[0x74] = 0040:00F5
    mov dword [mouse_irq_installed], 1
.attach_done:
    mov eax, 1                              ; success -> keep JLM resident
    ret 12
.attach_conflict:
    pop edi
    xor eax, eax
    ret 12
.check_detach:
    cmp eax, DLL_PROCESS_DETACH
    jne .ok
    call _sb_remove_traps
    call gate1_sf2_unload
    call gate1_audio_shutdown
    call _joy_remove_hooks
    call gate1_sd_shutdown
    call gate1_ehci_shutdown
    call gate1_usb_shutdown
    call gate1_wifi_disconnect
    ; Hand INT 60h back to the vector that preceded the packet stub,
    ; recorded through the AX=0x0007 API by NETINIT and WCONN alike.
    ; This must only happen while the vector still points at a Ferric
    ; stub -- identified by the "PKT DRVR" signature at +3 and the
    ; own-segment word the installer patched at +0x0C -- because a
    ; driver that re-hooked INT 60h after us owns the vector now and
    ; restoring over it would orphan that driver instead.
    cmp dword [net_old_vec60_saved], 0
    je .vec60_done
    mov eax, [0x00000180]               ; IVT entry for INT 60h
    mov edx, eax
    shr edx, 16                         ; handler segment
    and eax, 0xFFFF                     ; handler offset (installers use 0)
    shl edx, 4
    add edx, eax                        ; linear address of the handler
    cmp dword [edx + 3], 0x20544B50     ; "PKT " at +3
    jne .vec60_done
    cmp dword [edx + 7], 0x52565244     ; "DRVR" at +7
    jne .vec60_done
    mov eax, [0x00000180]
    shr eax, 16
    cmp word [edx + 0x0C], ax           ; stub's own-segment word
    jne .vec60_done
    mov eax, [net_old_vec60]
    mov [0x00000180], eax
    mov byte [edx], 0xCF                ; stub starts with IRET now, so a
                                        ; client still holding its address
                                        ; returns harmlessly instead of
                                        ; bridging into an unloaded JLM
.vec60_done:
    mov dword [net_old_vec60], 0
    mov dword [net_old_vec60_saved], 0
    mov dword [net_in_upcall], 0
    mov dword [sb_in_irq], 0
    mov dword [mouse_in_cb], 0
    cmp dword [net_installed], 0
    je .detach_mouse
    mov eax, PKT_BRIDGE_VECTOR
    mov esi, _pkt_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    mov edi, [net_stub_seg]
    test edi, edi
    jz .detach_net_clear
    shl edi, 4
    mov byte [edi], 0xCF                ; stub's first byte -> IRET
.detach_net_clear:
    mov dword [net_installed], 0
    ; The two DOS blocks are deliberately not freed: JLM unload is a
    ; teardown path, and a freed block that a stray client still
    ; reaches through a cached vector is worse than 17 KiB left
    ; self-owned until reboot.
.detach_mouse:
    cmp dword [mouse_irq_installed], 0
    je .detach_int33
    mov eax, [mouse_old_ivt74]
    mov [0x000001D0], eax
    mov eax, MOUSE_IRQ_BRIDGE_VECTOR
    mov esi, _mouse_irq_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    mov dword [mouse_irq_installed], 0
.detach_int33:
    cmp dword [mouse_ivt_patched], 0
    je .detach_int33_hook
    mov eax, [mouse_old_ivt33]
    mov [0x000000CC], eax
    mov dword [mouse_ivt_patched], 0
.detach_int33_hook:
    cmp dword [mouse_hooks_installed], 0
    je .detach_irq11
    mov eax, MOUSE_BRIDGE_VECTOR
    mov esi, _mouse_bridge_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    mov eax, INT33_VECTOR
    mov esi, _int33_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    mov dword [mouse_hooks_installed], 0
.detach_irq11:
    mov eax, TIMER_IRQ_VECTOR
    mov esi, _timer_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    mov eax, IRQ11_VECTOR
    mov esi, _irq11_hookproc
    VMMCall VMM_UNHOOK_V86_INT_CHAIN
    call gate1_svc_reset
.ok:
    mov eax, 1
    ret 12
