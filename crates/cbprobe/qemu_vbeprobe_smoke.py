"""QEMU TCG smoke test for Stage G Step 1: VBEPROBE.COM under `-vga std`
(Bochs VBE 2.0/3.0 BIOS) in Profile 2 (Connected: JemmEx + HDPMI32i).
"""

import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="vbeprobe-") as td:
        td_path = Path(td)
        test_img = td_path / "floppy.img"
        shutil.copy2(base_img, test_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"MTEST.COM STATUS\r\n",
                b"MTEST.COM STATUS\r\n"
                b"VBEPROBE.COM > COM1\r\n"
                b"ECHO VBEPROBE_DONE > COM1\r\n",
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

        log = td_path / "vbeprobe.log"
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
                    if "VBEPROBE_DONE" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("VBEPROBE banner printed", "Ferric DOS VBEPROBE" in text),
            ("VBE 2.0+ version reported", "VBEPROBE: ver=" in text and "vram=" in text),
            ("OEM string reported", "VBEPROBE: oem=" in text),
            ("800x600 LFB mode enumerated", "800x600x" in text and "LFB" in text),
            ("1024x768 LFB mode enumerated", "1024x768x" in text),
            ("summary line printed", "VBEPROBE: summary gfx_modes=" in text and "lfb_modes=" in text),
            ("guest reached end cleanly", "VBEPROBE_DONE" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[vbeprobe] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
