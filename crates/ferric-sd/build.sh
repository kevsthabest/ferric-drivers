#!/bin/sh
set -eu

# Build standalone Toshiba TC6371AF SD Slot JLM (out/TOSHSD.JLM, DDB 0xFD06).
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT_DIR}"

OUT_DIR="${ROOT_DIR}/out"
mkdir -p "${OUT_DIR}"

echo "[ferric-sd] 1/5 Building freestanding 32-bit Rust SD module (i386-unknown-none)..."
cargo build --release -p ferric-sd --target targets/i386-unknown-none.json

LIB_A="target/i386-unknown-none/release/libferric_sd.a"
JLM_RUST_OBJ="${OUT_DIR}/ferric_sd_jlm.o"
JLM_RUST_COFF="${OUT_DIR}/ferric_sd_jlm_coff.obj"
JLM_STUB_OBJ="${OUT_DIR}/sd_jlm_stub.obj"
JLM_DLL="${OUT_DIR}/TOSHSD.DLL"
JLM_PX="${OUT_DIR}/TOSHSD.JLM"

echo "[ferric-sd] 2/5 Partial-linking Rust object and verifying 386 purity..."
ld -m elf_i386 -r --gc-sections -T crates/gate1-svc/jlm_merge.ld     -u gate1_sd_init -u gate1_sd_read_sec -u gate1_sd_shutdown     "${LIB_A}" -o "${JLM_RUST_OBJ}"
python3 scripts/check_386_purity.py --require-code "${JLM_RUST_OBJ}"

echo "[ferric-sd] 3/5 Converting ELF32->COFF and assembling sd_jlm.asm..."
python3 crates/gate1-svc/elf32_to_coff.py "${JLM_RUST_OBJ}" "${JLM_RUST_COFF}"
nasm -f win32 -o "${JLM_STUB_OBJ}" crates/ferric-sd/asm/sd_jlm.asm
python3 scripts/check_asm_purity.py crates/ferric-sd/asm/sd_jlm.asm

echo "[ferric-sd] 4/5 Linking relocatable PE32 JLM out/TOSHSD.DLL and PX-patching..."
wlink format windows nt dll runtime native     file "${JLM_STUB_OBJ},${JLM_RUST_COFF}"     name "${JLM_DLL}"     option start=DllMain,quiet     export _ddb.1
python3 - "${JLM_DLL}" "${JLM_PX}" <<'PY'
import struct, sys
data = bytearray(open(sys.argv[1], "rb").read())
pe_off = struct.unpack_from("<I", data, 0x3C)[0]
assert data[pe_off : pe_off + 4] == b"PE\x00\x00", f"unexpected PE sig at {pe_off:#x}"
data[pe_off : pe_off + 4] = b"PX\x00\x00"
open(sys.argv[2], "wb").write(data)
PY

echo "[ferric-sd] 5/5 Verifying read-only guardrail..."
python3 scripts/check_readonly.py crates/ferric-sd
python3 scripts/check_readonly.py --disasm "${JLM_DLL}"

JLM_SIZE="$(wc -c < "${JLM_PX}")"
echo "[ferric-sd] Built ${JLM_PX} (${JLM_SIZE} B, PX-patched)"
sha256sum "${JLM_DLL}" "${JLM_PX}"
