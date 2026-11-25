#!/bin/sh
set -eu

# Build standalone Stage G VBE + 2D Acceleration JLM (out/VBE.JLM, DDB 0xFD04)
# inside ferric-dos-build:gate1.
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT_DIR}"

OUT_DIR="${ROOT_DIR}/out"
mkdir -p "${OUT_DIR}"

echo "[ferric-vbe] 1/5 Building freestanding 32-bit Rust VBE module (i386-unknown-none)..."
cargo build --release -p ferric-vbe --target targets/i386-unknown-none.json

LIB_A="target/i386-unknown-none/release/libferric_vbe.a"
JLM_RUST_OBJ="${OUT_DIR}/ferric_vbe_jlm.o"
JLM_RUST_COFF="${OUT_DIR}/ferric_vbe_jlm_coff.obj"
JLM_STUB_OBJ="${OUT_DIR}/vbe_jlm_stub.obj"
JLM_DLL="${OUT_DIR}/VBE.DLL"
JLM_PX="${OUT_DIR}/VBE.JLM"

echo "[ferric-vbe] 2/5 Partial-linking Rust object and verifying 386 purity..."
ld -m elf_i386 -r --gc-sections -T crates/gate1-svc/jlm_merge.ld \
    -u vbe_jlm_init -u vbe_jlm_get_caps -u vbe_jlm_set_lfb \
    -u vbe_jlm_fill_rect -u vbe_jlm_bit_blt -u vbe_jlm_sync -u vbe_jlm_release_hw \
    "${LIB_A}" -o "${JLM_RUST_OBJ}"
python3 scripts/check_386_purity.py --require-code "${JLM_RUST_OBJ}"

echo "[ferric-vbe] 3/5 Converting ELF32->COFF and assembling vbe_jlm.asm..."
python3 crates/gate1-svc/elf32_to_coff.py "${JLM_RUST_OBJ}" "${JLM_RUST_COFF}"
nasm -f win32 -o "${JLM_STUB_OBJ}" crates/ferric-vbe/asm/vbe_jlm.asm
python3 scripts/check_asm_purity.py crates/ferric-vbe/asm/vbe_jlm.asm

echo "[ferric-vbe] 4/5 Linking relocatable PE32 JLM out/VBE.DLL and PX-patching..."
wlink format windows nt dll runtime native \
    file "${JLM_STUB_OBJ},${JLM_RUST_COFF}" \
    name "${JLM_DLL}" \
    option start=DllMain,quiet \
    export _ddb.1
python3 - "${JLM_DLL}" "${JLM_PX}" <<'PY'
import struct, sys
data = bytearray(open(sys.argv[1], "rb").read())
pe_off = struct.unpack_from("<I", data, 0x3C)[0]
assert data[pe_off : pe_off + 4] == b"PE\x00\x00", f"unexpected PE sig at {pe_off:#x}"
data[pe_off : pe_off + 4] = b"PX\x00\x00"
open(sys.argv[2], "wb").write(data)
PY

echo "[ferric-vbe] 5/5 Verifying read-only guardrail..."
python3 scripts/check_readonly.py crates/ferric-vbe
python3 scripts/check_readonly.py --disasm "${JLM_DLL}"

JLM_SIZE="$(wc -c < "${JLM_PX}")"
echo "[ferric-vbe] Built ${JLM_PX} (${JLM_SIZE} B, PX-patched)"
sha256sum "${JLM_DLL}" "${JLM_PX}"
