#!/usr/bin/env python3
"""Boot a DOS floppy image under QEMU TCG and dump the VGA text screen.

Diagnostic helper for Gate 1: CONFIG.SYS driver messages (JEMMEX, JLOAD) go to
the VGA console, not to COM1, so the serial log alone cannot show them. This
boots the image headless (-display none, TCG only), waits, then reads the
80x25 text buffer at 0xB8000 through the QEMU monitor (pmemsave) and prints it.

Usage: qemu_screen.py IMAGE [--cpu pentium3] [--wait 20] [--serial LOG]
"""

from __future__ import annotations

import argparse
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def monitor_cmd(sock_path: str, cmd: str) -> None:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock_path)
    s.settimeout(3)
    try:
        s.recv(4096)
    except OSError:
        pass
    s.sendall((cmd + "\n").encode())
    time.sleep(0.5)
    try:
        s.recv(65536)
    except OSError:
        pass
    s.close()


def dump_screen(raw: bytes) -> str:
    lines = []
    for row in range(25):
        chars = raw[row * 160:(row + 1) * 160:2]
        lines.append(chars.decode("cp437", errors="replace").rstrip())
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("image")
    ap.add_argument("--cpu", default="pentium3")
    ap.add_argument("--wait", type=float, default=20.0)
    ap.add_argument("--serial", default=None)
    ap.add_argument("--mode", choices=("floppy", "hdd"), default="floppy")
    args = ap.parse_args()

    with tempfile.TemporaryDirectory(prefix="qscr-") as td:
        mon = str(Path(td) / "mon.sock")
        dump = str(Path(td) / "vga.bin")
        serial = args.serial or str(Path(td) / "serial.log")
        if args.mode == "floppy":
            drive = [f"-drive", f"file={args.image},format=raw,if=floppy,snapshot=on", "-boot", "a"]
        else:
            drive = [f"-drive", f"file={args.image},format=raw,if=ide,snapshot=on", "-boot", "c"]
        cmd = [
            "qemu-system-i386", "-machine", "pc", "-accel", "tcg", "-m", "64",
            "-cpu", args.cpu, "-display", "none", "-no-reboot",
            "-serial", f"file:{serial}",
            "-monitor", f"unix:{mon},server,nowait",
            *drive,
        ]
        if args.cpu == "pentium3":
            cmd += ["-netdev", "user,id=net0", "-device", "i82550,netdev=net0", "-device", "pci-ohci"]
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            time.sleep(args.wait)
            if proc.poll() is not None:
                print("QEMU exited early:", proc.stderr.read().decode(errors="replace"), file=sys.stderr)
            else:
                monitor_cmd(mon, f"pmemsave 0xB8000 4000 {dump}")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
        if Path(dump).exists():
            print("----- VGA text screen -----")
            print(dump_screen(Path(dump).read_bytes()))
        if Path(serial).exists():
            print("----- serial log -----")
            print(Path(serial).read_text(encoding="latin-1", errors="replace"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
