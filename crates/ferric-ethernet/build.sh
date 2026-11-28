#!/bin/sh
set -eu

# Build standalone Intel Ethernet JLM (out/I8255X.JLM, DDB 0xFD08).
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT_DIR}"

OUT_DIR="${ROOT_DIR}/out"
mkdir -p "${OUT_DIR}"
nasm -f bin -o out/pktstub.bin crates/gate1-svc/asm/pktstub.asm
nasm -f bin -o out/ETHINIT.COM crates/ferric-ethernet/asm/ethinit.asm

echo "[ferric-ethernet] 1/5 Building freestanding 32-bit Rust SD module (i386-unknown-none)..."
cargo build --release -p ferric-ethernet --target targets/i386-unknown-none.json

LIB_A="target/i386-unknown-none/release/libferric_ethernet.a"
JLM_RUST_OBJ="${OUT_DIR}/ferric_ethernet_jlm.o"
JLM_RUST_COFF="${OUT_DIR}/ferric_ethernet_jlm_coff.obj"
JLM_STUB_OBJ="${OUT_DIR}/eth_jlm_stub.obj"
JLM_DLL="${OUT_DIR}/I8255X.DLL"
JLM_PX="${OUT_DIR}/I8255X.JLM"

echo "[ferric-ethernet] 2/5 Partial-linking Rust object and verifying 386 purity..."
ld -m elf_i386 -r --gc-sections -T crates/gate1-svc/jlm_merge.ld     -u gate1_net_probe -u gate1_net_init -u gate1_net_stub_info -u gate1_net_stub_copy -u gate1_net_api -u gate1_net_service -u gate1_net_upcall_next -u gate1_net_upcall_after -u eth_irq -u eth_poll -u eth_shutdown -u eth_release_resources     "${LIB_A}" -o "${JLM_RUST_OBJ}"
python3 scripts/check_386_purity.py --require-code "${JLM_RUST_OBJ}"

echo "[ferric-ethernet] 3/5 Converting ELF32->COFF and assembling eth_jlm.asm..."
python3 crates/gate1-svc/elf32_to_coff.py "${JLM_RUST_OBJ}" "${JLM_RUST_COFF}"
nasm -f win32 -o "${JLM_STUB_OBJ}" crates/ferric-ethernet/asm/eth_jlm.asm
python3 scripts/check_asm_purity.py crates/ferric-ethernet/asm/eth_jlm.asm

echo "[ferric-ethernet] 4/5 Linking relocatable PE32 JLM out/I8255X.DLL and PX-patching..."
wlink format windows nt dll runtime native     file "${JLM_STUB_OBJ},${JLM_RUST_COFF}"     name "${JLM_DLL}"     option start=DllMain,quiet     export _ddb.1
python3 - "${JLM_DLL}" "${JLM_PX}" <<'PY'
import struct, sys
data = bytearray(open(sys.argv[1], "rb").read())
pe_off = struct.unpack_from("<I", data, 0x3C)[0]
assert data[pe_off : pe_off + 4] == b"PE\x00\x00", f"unexpected PE sig at {pe_off:#x}"
data[pe_off : pe_off + 4] = b"PX\x00\x00"
open(sys.argv[2], "wb").write(data)
PY

echo "[ferric-ethernet] 5/5 Verifying read-only guardrail..."
python3 scripts/check_readonly.py crates/ferric-ethernet
python3 scripts/check_readonly.py --disasm "${JLM_DLL}"

JLM_SIZE="$(wc -c < "${JLM_PX}")"
echo "[ferric-ethernet] Built ${JLM_PX} (${JLM_SIZE} B, PX-patched)"
sha256sum "${JLM_DLL}" "${JLM_PX}"
