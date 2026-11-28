#!/bin/sh
# Run in the pinned driver build container, after scripts/build-drivers.sh.
# Upstream FreeDOS inputs are pinned; nothing from the private DOS repo needed.
set -eu
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p out/ethernet-qemu
curl -fsSL https://github.com/FDOS/kernel/releases/download/ke2046/ke2046_386f32.zip -o "$tmp/kernel.zip"
echo "9305ba9140c6ab0287612b5898c250756c6c6953204c3bda802b39ef853d372c  $tmp/kernel.zip" | sha256sum -c -
curl -fsSL https://github.com/FDOS/kernel/releases/download/ke2046/ke2046s.zip -o "$tmp/source.zip"
echo "0ea71c9276aba7e1e371f42e8e17d4e9223ac4bedeacccce3ee700fc2866c5e3  $tmp/source.zip" | sha256sum -c -
python3 - "$tmp" <<'PY'
import sys, zipfile
from pathlib import Path
p = Path(sys.argv[1])
for archive, name, dest in [
    ("kernel.zip", "kernel.sys", "KERNEL.SYS"),
    ("source.zip", "boot/boot.asm", "boot.asm"),
    ("source.zip", "boot/magic.mac", "magic.mac"),
]:
    with zipfile.ZipFile(p / archive) as z:
        names = [n for n in z.namelist() if n.lower().endswith("/" + name)]
        if len(names) != 1:
            raise SystemExit(f"{archive}: expected unique {name}, found {names}")
        (p / dest).write_bytes(z.read(names[0]))
PY
(cd "$tmp"; nasm -f bin -dISFAT12 boot.asm -o boot.bin)
dd if=/dev/zero of="$tmp/base.img" bs=512 count=2880 status=none
mkfs.fat -F 12 -n ETHSMOKE "$tmp/base.img" >/dev/null
python3 - "$tmp/base.img" "$tmp/boot.bin" <<'PY'
from pathlib import Path
import sys
image, boot = map(Path, sys.argv[1:])
b = boot.read_bytes()
assert len(b) == 512
with image.open("r+b") as f:
    sector = bytearray(f.read(512))
    sector[:3] = b[:3]
    sector[62:] = b[62:]
    sector[0x24] = 0
    f.seek(0)
    f.write(sector)
PY
mcopy -i "$tmp/base.img" "$tmp/KERNEL.SYS" ::/KERNEL.SYS
mcopy -i "$tmp/base.img" /opt/freedos/command.com ::/COMMAND.COM
mcopy -i "$tmp/base.img" /opt/freedos/jemm/JEMMEX.EXE /opt/freedos/jemm/JLOAD.EXE ::/
mcopy -i "$tmp/base.img" out/FRES.JLM out/I8255X.JLM out/ETHINIT.COM out/NTEST.COM out/CARDBUS.JLM out/ATH5K.JLM out/ATHDIAG.COM out/ATHINIT.COM out/ATHSCAN.COM out/ATHCONN.COM out/ATHSTOP.COM out/CBPROBE.COM out/WIFIGATE.BAT out/PCIEVID.COM out/CBSTART.COM out/CBSTOP.COM out/PNPEVID.COM ::/
printf 'DEVICE=JEMMEX.EXE NOEMS D=64\r\nFILES=20\r\nBUFFERS=10\r\nLASTDRIVE=E\r\nSHELL=COMMAND.COM /P\r\n' > "$tmp/CONFIG.SYS"
mcopy -i "$tmp/base.img" "$tmp/CONFIG.SYS" ::/
python3 scripts/ethernet-qemu.py "$tmp/base.img" out/ethernet-qemu
