#!/usr/bin/env python3
"""Boot standalone diagnostics and the resident Gate 1 profile in QEMU TCG."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REQUIRED_MARKERS = (
    "=== FERRIC-DOS GATE1 BEGIN ===",
    "GATE1:SVC ",
    "GATE1:IRQ11 ",
    "GATE1:DMA ",
    "GATE1:COEXIST ",
    "=== FERRIC-DOS GATE1 END ===",
)

PROFILES = (
    ("486", ["-cpu", "486", "-nic", "none"]),
    ("pentium", ["-cpu", "pentium", "-nic", "none"]),
    (
        "pentium3",
        [
            "-cpu",
            "pentium3",
            "-netdev",
            "user,id=net0",
            "-device",
            "i82550,netdev=net0",
            "-device",
            "pci-ohci",
        ],
    ),
)


def run_profile(name: str, extra_args: list[str], img_path: Path, log_path: Path, timeout_s: float, markers=REQUIRED_MARKERS) -> bool:
    if log_path.exists():
        log_path.unlink()
    cmd = [
        "qemu-system-i386",
        "-machine",
        "pc",
        "-accel",
        "tcg",
        "-m",
        "64",
        "-display",
        "none",
        "-serial",
        f"file:{log_path}",
        "-no-reboot",
        *extra_args,
        "-drive",
        f"file={img_path},format=raw,if=floppy,snapshot=on",
        "-boot",
        "a",
    ]
    t0 = time.monotonic()
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    text = ""
    try:
        while time.monotonic() - t0 < timeout_s:
            if log_path.exists():
                text = log_path.read_text(encoding="latin-1", errors="replace")
                if markers[-1] in text:
                    break
            if proc.poll() is not None:
                break
            time.sleep(0.2)
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()

    elapsed = time.monotonic() - t0
    if log_path.exists():
        text = log_path.read_text(encoding="latin-1", errors="replace")

    missing = [m for m in markers if m not in text]
    has_fail = "status=FAIL" in text
    ok = (not missing) and (not has_fail)
    status = "PASS" if ok else "FAIL"
    print(f"[gate1-qemu] {name:10s} {status} ({elapsed:.2f}s)")
    for line in text.splitlines():
        if line.strip():
            print(f"  | {line}")
    if missing:
        print(f"  MISSING MARKERS: {missing}", file=sys.stderr)
    if has_fail:
        print("  ERROR: status=FAIL detected in Gate 1 output", file=sys.stderr)
    return ok


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="gate1-qemu-") as td:
        td_path = Path(td)
        all_ok = True
        for mode in ("standalone", "resident"):
            test_img = td_path / f"{mode}.img"
            shutil.copy2(base_img, test_img)
            autoexec = td_path / "AUTOEXEC.BAT"
            markers = REQUIRED_MARKERS
            if mode == "standalone":
                autoexec.write_bytes(b"@ECHO OFF\r\nAPMSTAT.COM > COM1\r\nSVCTEST.COM\r\n")
                markers = ("APMSTAT: present=1",) + REQUIRED_MARKERS
            else:
                # Select the shipped menu entry and capture the DOS client's stdout.
                for filename, before, after in (
                    ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=1,1"),
                    ("AUTOEXEC.BAT", b"\r\nG1LIVE.COM\r\n", b"\r\nG1LIVE.COM > COM1\r\n"),
                ):
                    path = td_path / filename
                    subprocess.run(["mcopy", "-o", "-i", str(test_img), "::/" + filename, str(path)], check=True)
                    data = path.read_bytes()
                    if data.count(before) != 1:
                        raise ValueError(f"Expected one {before!r} in {filename}")
                    path.write_bytes(data.replace(before, after))
                    subprocess.run(["mcopy", "-o", "-i", str(test_img), str(path), "::/" + filename], check=True)
                markers = (
                    "GATE1:MEM largest=0x",
                    "GATE1:IRQHOOK status=PASS",
                    "GATE1:DMARESIDENT status=PASS",
                    "crc=0x0155C515",
                    "GATE1:LIVE PASS V86 and DPMI called FERRIC.JLM",
                )
            if mode == "standalone":
                subprocess.run(["mcopy", "-o", "-i", str(test_img), str(autoexec), "::/AUTOEXEC.BAT"], check=True)
            for name, extra in PROFILES:
                label = f"{mode}-{name}"
                if not run_profile(label, extra, test_img, td_path / f"{label}.log", 30.0, markers):
                    all_ok = False
        return 0 if all_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
