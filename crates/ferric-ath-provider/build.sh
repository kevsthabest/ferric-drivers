#!/bin/sh
set -eu

# Build standalone Atheros resident JLM (out/ATH5K.JLM, DDB 0xFD0A).
ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${ROOT_DIR}"

OUT_DIR="${ROOT_DIR}/out"
mkdir -p "${OUT_DIR}"



echo "[ferric-ath-provider] 1/5 Building freestanding 32-bit Rust Atheros module (i386-unknown-none)..."
cargo build --release -p ferric-ath-provider --target targets/i386-unknown-none.json

LIB_A="target/i386-unknown-none/release/libferric_ath_provider.a"
JLM_RUST_OBJ="${OUT_DIR}/ferric_ath_provider_jlm.o"
JLM_RUST_COFF="${OUT_DIR}/ferric_ath_provider_jlm_coff.obj"
JLM_STUB_OBJ="${OUT_DIR}/ath_jlm_stub.obj"
JLM_DLL="${OUT_DIR}/ATH5K.DLL"
JLM_PX="${OUT_DIR}/ATH5K.JLM"

echo "[ferric-ath-provider] 2/5 Partial-linking Rust object and verifying 386 purity..."
ld -m elf_i386 -r --gc-sections -T crates/gate1-svc/jlm_merge.ld     -u ath_version -u ath_probe -u ath_shutdown -u ath_query "${LIB_A}" -o "${JLM_RUST_OBJ}"
python3 scripts/check_386_purity.py --require-code "${JLM_RUST_OBJ}"

echo "[ferric-ath-provider] 3/5 Converting ELF32->COFF and assembling ath_jlm.asm..."
python3 crates/gate1-svc/elf32_to_coff.py "${JLM_RUST_OBJ}" "${JLM_RUST_COFF}"
nasm -f win32 -o "${JLM_STUB_OBJ}" crates/ferric-ath-provider/asm/ath_jlm.asm
python3 scripts/check_asm_purity.py crates/ferric-ath-provider/asm/ath_jlm.asm

echo "[ferric-ath-provider] 4/5 Linking relocatable PE32 JLM out/ATH5K.DLL and PX-patching..."
wlink format windows nt dll runtime native     file "${JLM_STUB_OBJ},${JLM_RUST_COFF}"     name "${JLM_DLL}"     option start=DllMain,quiet     export _ddb.1
python3 - "${JLM_DLL}" "${JLM_PX}" <<'PY'
import struct, sys
data = bytearray(open(sys.argv[1], "rb").read())
pe_off = struct.unpack_from("<I", data, 0x3C)[0]
assert data[pe_off : pe_off + 4] == b"PE\x00\x00", f"unexpected PE sig at {pe_off:#x}"
data[pe_off : pe_off + 4] = b"PX\x00\x00"
open(sys.argv[2], "wb").write(data)
PY

echo "[ferric-ath-provider] 5/5 Verifying read-only guardrail..."
python3 scripts/check_readonly.py crates/ferric-ath-provider
python3 scripts/check_readonly.py --disasm "${JLM_DLL}"

JLM_SIZE="$(wc -c < "${JLM_PX}")"
echo "[ferric-ath-provider] Built ${JLM_PX} (${JLM_SIZE} B, PX-patched)"
sha256sum "${JLM_DLL}" "${JLM_PX}"
