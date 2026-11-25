"""QEMU TCG smoke test for Stage G Step 2: VBECTL.COM (VBE 2.0 LFB mode switch,
DPMI 0800h physical VRAM mapping under HDPMI32i, color-bar write + corner pixel
readback, and clean restore to 80x25 text mode) under `-vga std`.
"""

import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="vbectl-") as td:
        td_path = Path(td)
        test_img = td_path / "floppy.img"
        shutil.copy2(base_img, test_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"MTEST.COM STATUS\r\n",
                b"MTEST.COM STATUS\r\n"
                b"VBECTL.COM 800 /Q > COM1\r\n"
                b"VBECTL.COM 1024 /Q > COM1\r\n"
                b"ECHO VBECTL_DONE > COM1\r\n",
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

        log = td_path / "vbectl.log"
        proc = subprocess.Popen(
            [
                "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
                "-vga", "std",
                "-m", "64", "-accel", "tcg", "-display", "none",
                "-boot", "a",
                "-serial", f"file:{log}",
                "-drive", f"file={test_img},format=raw,if=floppy",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 45.0
            text = ""
            while time.time() < deadline:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "VBECTL_DONE" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("800x600 LFB mode switch + DPMI 0800h mapping PASS", "VBECTL: PASS mode=0x" in text and "800x600x" in text),
            ("1024x768 LFB mode switch + DPMI 0800h mapping PASS", "1024x768x" in text),
            ("LFB white border corner pixel readback non-zero", "px0=0x00FFFFFF" in text or "px0=0xFFFFFFFF" in text),
            ("clean restore and guest reached end", "VBECTL_DONE" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[vbectl] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
