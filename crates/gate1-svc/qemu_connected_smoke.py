#!/usr/bin/env python3
"""Boot the Connected profile (menu 2) in QEMU TCG and verify the
Gate 2 step 2.0 scaffold: the resident substrate loads, G1LIVE passes
(including the conventional-memory figure the profile's footprint is
judged against), APMSTAT reports, and the mouse driver answers its
one-shot status through INT 33h. All three reports are redirected to
COM1 in the test copy of AUTOEXEC.BAT; the shipped file prints them
to the console for the operator.
"""
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="gate2-connected-") as td:
        td_path = Path(td)
        test_img = td_path / "connected.img"
        shutil.copy2(base_img, test_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"\r\nG1LIVE\r\nIF ERRORLEVEL 1 GOTO FAILED\r\nNETINIT.COM\r\nAPMSTAT.COM\r\nMTEST.COM STATUS\r\n",
                b"\r\nG1LIVE.COM > COM1\r\nIF ERRORLEVEL 1 GOTO FAILED\r\nNETINIT.COM\r\nAPMSTAT.COM > COM1\r\nMTEST.COM STATUS > COM1\r\n",
            ),
        ):
            path = td_path / filename
            subprocess.run(
                ["mcopy", "-o", "-i", str(test_img), "::/" + filename, str(path)],
                check=True,
            )
            data = path.read_bytes()
            if data.count(before) != 1:
                raise ValueError(f"Expected one {before!r} in {filename}")
            path.write_bytes(data.replace(before, after))
            subprocess.run(
                ["mcopy", "-o", "-i", str(test_img), str(path), "::/" + filename],
                check=True,
            )

        log = td_path / "connected.log"
        proc = subprocess.Popen(
            [
                "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
                "-m", "256", "-accel", "tcg", "-display", "none",
                "-serial", f"file:{log}",
                "-drive", f"file={test_img},format=raw,if=floppy",
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 40.0
            text = ""
            while time.time() < deadline:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "MTEST: present=" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("resident substrate live", "GATE1:LIVE PASS" in text),
            ("APM status reported", "APMSTAT: present=1" in text),
            ("mouse driver present", "MTEST: present=1" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[gate2-connected] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1200:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
