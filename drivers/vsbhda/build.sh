#!/bin/sh
# Run in the Ferric build container with pinned source trees mounted read-only.
set -eu
src=$(cd "$(dirname "$0")" && pwd)
out=$1
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -R /jwasm "$work/jwasm"
make -s -C "$work/jwasm" -f GccUnix.mak -j4
asm=$(find "$work/jwasm" -type f -name jwasm | head -1)
cp -R /vsbhda "$work/vsb"
cd "$work/vsb"
export WATCOM=/opt/watcom
export PATH="$WATCOM/binl64:$PATH"
mkdir -p owdrv ow
# The DOS startup objects use 32-bit tiny model and share the module's DS.
for obj in LIBMAIN32 DPRINTF VIOOUT MALLOC SBRK; do
    "$asm" -q -zcw '-D?MODEL=tiny' -Isrcdrv/startup -Foowdrv/$obj.obj srcdrv/startup/$obj.ASM
done
for obj in DPMI30X DPMI507 DPMI80X; do
    "$asm" -q -Foowdrv/$obj.obj srcdrv/$obj.ASM
done
for obj in PHYSMEM DMABUFF PCIBIOS AC97MIX; do
    make -s -f LinuxDrv.mak CC=wcc386 owdrv/$obj.obj
done
wcc386 -q -oxa -ms -ecc -5s -fp5 -fpi87 -wcd=111 -Isrcdrv -Iinclude -I"$WATCOM/h" -fo=ali.obj "$src/ALI5451.C"
wlib -q -b -n helpers.lib owdrv/*.obj
wlink format dos file ali.obj,owdrv/LIBMAIN32.obj name ALI5451.DRV \
    alias __sndcard_info=_ALI5451_sndcard_info lib helpers.lib,"$WATCOM/lib386/dos/clib3s.lib" op q,nodefaultlibs disable 1014
# Upstream uses a lowercase stub path in Linux.mak.
ln -sf RES res
make -s -f Linux.mak ow/VSBHDA.EXE CC=wcc386 CPP=wpp386 LIB=wlib LINK=wlink ASM="$asm" USEJWL=0
nasm -f bin "$src/prep.asm" -o VSBPREP.COM
# Match upstream's PATCHPE step for WLink. FreeDOS uses the HX DOS stub
# for PX files; an ordinary PE signature gives DOS loader error 11.
python3 - <<'PYBUILD'
from pathlib import Path
from struct import unpack_from
p = Path("ow/VSBHDA.EXE")
b = bytearray(p.read_bytes())
offset, = unpack_from("<I", b, 0x3c)
assert b[offset:offset+4] == b"PE\0\0"
b[offset+1] = ord("X")
p.write_bytes(b)
drv = Path("ALI5451.DRV").read_bytes()
assert drv[:2] == b"MZ" and unpack_from("<H", drv, 6)[0] == 0
PYBUILD
# Build the matching JLM in isolation from the default image outputs.
mkdir "$work/ferric"
(cd "$src/../.." && tar -cf - .cargo Cargo.toml Cargo.lock crates scripts targets rust-toolchain.toml) | tar -xf - -C "$work/ferric"
(cd "$work/ferric" && sh crates/gate1-svc/build.sh)
cp ow/VSBHDA.EXE ALI5451.DRV VSBPREP.COM "$work/ferric/out/FERRIC.JLM" /opt/freedos/jemm/QPIEMU.DLL "$out/"

