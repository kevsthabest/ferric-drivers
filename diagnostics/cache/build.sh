#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
mkdir -p out/cache-diagnostic
nasm -f win32 diagnostics/cache/cache_jlm.asm -o out/cache-diagnostic/cache.obj
wlink format windows nt dll runtime native file out/cache-diagnostic/cache.obj name out/cache-diagnostic/CACHE.DLL option start=DllMain,quiet export _ddb.1
python3 - <<'PY'
from pathlib import Path
import struct
p=Path("out/cache-diagnostic/CACHE.DLL")
b=bytearray(p.read_bytes())
off=struct.unpack_from("<I",b,0x3c)[0]
assert b[off:off+4]==b"PE\0\0"
b[off:off+4]=b"PX\0\0"
p.with_suffix(".JLM").write_bytes(b)
PY
