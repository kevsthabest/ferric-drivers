#!/bin/sh
set -eu

# Build Gate 1 Track B deliverables inside ferric-dos-build:gate0 (zero host paths):
#   - out/SVCTEST.COM : Standalone 16-bit RM + 32-bit PM resident service harness
#   - out/FERRIC.DLL  : Relocatable PE32 NT Native JLM module (JEMM386 + JLOAD)
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT_DIR}"

OUT_DIR="${ROOT_DIR}/out"
mkdir -p "${OUT_DIR}"

echo "[gate1-svc] 0/6 Assembling the packet driver's real-mode stub (embedded by the Rust core)..."
nasm -f bin -o "${OUT_DIR}/pktstub.bin" crates/gate1-svc/asm/pktstub.asm

echo "[gate1-svc] 1/6 Building freestanding 32-bit Rust service (i386-unknown-none)..."
cargo build --release -p gate1-svc --target targets/i386-unknown-none.json

LIB_A="${ROOT_DIR}/target/i386-unknown-none/release/libgate1_svc.a"
CORE_ELF="${OUT_DIR}/gate1_svc_core.elf"
CORE_BIN="${OUT_DIR}/gate1_svc_core.bin"
STUB_BIN="${OUT_DIR}/svctest_stub.bin"
COM_OUT="${OUT_DIR}/SVCTEST.COM"
JLM_RUST_OBJ="${OUT_DIR}/gate1_svc_jlm.o"
JLM_RUST_COFF="${OUT_DIR}/gate1_svc_jlm.obj"
JLM_STUB_OBJ="${OUT_DIR}/jlm_stub.obj"
JLM_DLL="${OUT_DIR}/FERRIC.DLL"
JLM_PX="${OUT_DIR}/FERRIC.JLM"

echo "[gate1-svc] 2/6 Linking 32-bit Rust core at VMA 0x1800 for SVCTEST.COM..."
ld -m elf_i386 --gc-sections -T crates/gate1-svc/linker.ld \
    -u gate1_svc_entry "${LIB_A}" -o "${CORE_ELF}"

echo "[gate1-svc] 3/6 Verifying 386 instruction purity on Rust core..."
python3 scripts/check_386_purity.py --require-code "${CORE_ELF}"

echo "[gate1-svc] 4/6 Packaging out/SVCTEST.COM..."
objcopy -O binary "${CORE_ELF}" "${CORE_BIN}"
CORE_SIZE="$(wc -c < "${CORE_BIN}")"
if [ "${CORE_SIZE}" -gt 20480 ]; then
    echo "FAIL: gate1_svc_core.bin (${CORE_SIZE} bytes) exceeds 20480-byte slot (0x1800..0x67FF)" >&2
    exit 1
fi
nasm -f bin -o "${STUB_BIN}" crates/gate1-svc/asm/svctest.asm
nasm -f bin -o out/G1LIVE.COM crates/gate1-svc/asm/g1live.asm
nasm -f bin -o out/NETINIT.COM crates/gate1-svc/asm/netinit.asm
nasm -f bin -o out/NTEST.COM crates/gate1-svc/asm/ntest.asm
cat "${STUB_BIN}" "${CORE_BIN}" > "${COM_OUT}"

echo "[gate1-svc] 5/6 Building relocatable PE32 JLM module out/FERRIC.DLL (NASM + wlink)..."
ld -m elf_i386 -r --gc-sections -T crates/gate1-svc/jlm_merge.ld \
    -u gate1_svc_entry -u gate1_irq11_isr -u gate1_svc_reset -u gate1_svc_set_platform_vdmad \
    -u gate1_mouse_init -u gate1_mouse_feed -u gate1_mouse_api -u gate1_mouse_cb_info -u gate1_mouse_present \
    -u gate1_net_probe -u gate1_net_init -u gate1_net_stub_info -u gate1_net_stub_copy -u gate1_net_api -u gate1_net_service -u gate1_net_poll_wifi -u gate1_net_upcall_next -u gate1_net_upcall_after \
    -u gate1_cb_init -u gate1_cb_slot_info -u gate1_wifi_probe -u gate1_wifi_scan -u gate1_wifi_connect \
    -u gate1_wifi_diag -u gate1_net_switch_backend -u gate1_wifi_disconnect \
    -u gate1_usb_init -u gate1_usb_shutdown -u gate1_usb_poll \
    -u gate1_usb_set_delay -u gate1_usb_get_delay -u gate1_usb_mouse_take_wheel \
    -u gate1_joy_io -u gate1_joy_present -u gate1_joy_int15 \
    -u gate1_usb_msc_dir -u gate1_usb_msc_open -u gate1_usb_msc_read_chunk \
    -u gate1_usb_msc_bpb -u gate1_usb_msc_read_sec \
    -u gate1_usb_msc_write_sec -u gate1_usb_msc_flush -u gate1_usb_msc_eject -u gate1_usb_msc_media_check \
    -u gate1_blk_abi_version -u gate1_blk_info -u gate1_usb_msc_fs_type \
    -u gate1_ehci_recon -u gate1_ehci_bringup -u gate1_ehci_shutdown \
    -u gate1_m7101_info -u gate1_m7101_set_throttle -u gate1_m7101_set_backlight \
    -u gate1_sd_init -u gate1_sd_read_sec -u gate1_sd_shutdown \
    -u gate1_sf2_begin -u gate1_sf2_write_chunk -u gate1_sf2_commit -u gate1_sf2_unload -u gate1_sf2_status \
    -u gate1_audio_external_prepare -u gate1_audio_init -u gate1_audio_chime -u gate1_audio_status -u gate1_audio_shutdown \
    -u gate1_audio_sb_start -u gate1_audio_irq11 -u gate1_audio_poll -u gate1_audio_set_vol -u gate1_sb_io -u gate1_sb_take_irq -u gate1_sb_take_up_to -u gate1_sb_take_one_irq -u gate1_sb_diag -u gate1_sb_dma_diag -u gate1_sb_mixer_ctl \
    "${LIB_A}" -o "${JLM_RUST_OBJ}"
python3 scripts/check_386_purity.py --require-code "${JLM_RUST_OBJ}"
python3 crates/gate1-svc/elf32_to_coff.py "${JLM_RUST_OBJ}" "${JLM_RUST_COFF}"
nasm -f win32 -o "${JLM_STUB_OBJ}" crates/gate1-svc/asm/jlm_stub.asm
wlink format windows nt dll runtime native \
    file "${JLM_STUB_OBJ},${JLM_RUST_COFF}" \
    name "${JLM_DLL}" \
    option start=DllMain,quiet \
    export _ddb.1
# No objdump sweep of the linked DLL: the Jemm hook ABI puts a
# data dword immediately before each hook proc, inside the code
# section, and a linear disassembly sweep manufactures phantom
# instructions from it (2026-11-21: an "fdivl" whose bytes are the
# middle of a call rel32). The DLL's Rust content is the object
# swept above, byte for byte; its only other input is the
# hand-written stub, checked exactly at source level.
python3 scripts/check_asm_purity.py crates/gate1-svc/asm/jlm_stub.asm crates/gate1-svc/asm/pktstub.asm
python3 - "${JLM_DLL}" "${JLM_PX}" <<'PY'
import struct, sys
data = bytearray(open(sys.argv[1], "rb").read())
pe_off = struct.unpack_from("<I", data, 0x3C)[0]
assert data[pe_off : pe_off + 4] == b"PE\x00\x00", f"unexpected PE sig at {pe_off:#x}"
data[pe_off : pe_off + 4] = b"PX\x00\x00"
open(sys.argv[2], "wb").write(data)
PY

echo "[gate1-svc] 6/6 Verifying read-only guardrail on crates/gate1-svc..."
python3 scripts/check_readonly.py crates/gate1-svc
python3 scripts/check_readonly.py --disasm "${CORE_ELF}"
python3 scripts/check_readonly.py --disasm "${JLM_DLL}"

COM_SIZE="$(wc -c < "${COM_OUT}")"
DLL_SIZE="$(wc -c < "${JLM_DLL}")"
JLM_SIZE="$(wc -c < "${JLM_PX}")"
echo "[gate1-svc] Built ${COM_OUT} (${COM_SIZE} B, rust_core=${CORE_SIZE} B), ${JLM_DLL} (${DLL_SIZE} B), ${JLM_PX} (${JLM_SIZE} B, PX-patched)"
sha256sum "${COM_OUT}" "${JLM_DLL}" "${JLM_PX}"
