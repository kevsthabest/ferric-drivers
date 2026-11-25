"""QEMU TCG smoke test for the exFAT read path (EXFAT.COM + JLM verbs
0x001B-0x001D over the OHCI USB mass-storage stack).

Builds a 16 MiB exFAT stick image with scripts/make_exfat_image.py
(4 KiB clusters; BIG.BIN scattered across three clusters by its FAT
chain), boots Profile 2 (Connected) with the stick on the emulated
OHCI hub, runs USBPROBE + EXFAT.COM, and asserts the directory listing
and byte-exact copies of a root file, the scattered file, a two-name-
entry long-named file, and a subdirectory file.
"""

import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import make_exfat_image as exfat_img


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="exfat-") as td:
        td_path = Path(td)
        test_img = td_path / "usb.img"
        shutil.copy2(base_img, test_img)

        stick_img = td_path / "stick.img"
        exfat_img.build_image(stick_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"MTEST.COM STATUS\r\n",
                b"MTEST.COM STATUS\r\n"
                b"USBPROBE.COM > COM1\r\n"
                b"FSLOAD.COM > COM1\r\n"
                b"EXFAT.COM > COM1\r\n"
                b"EXFAT.COM HELLO.TXT A:\\HELLO.TXT > COM1\r\n"
                b"EXFAT.COM BIG.BIN A:\\BIG.BIN > COM1\r\n"
                b"EXFAT.COM GAMES\\DOOM.TXT A:\\DOOM.TXT > COM1\r\n"
                b"EXFAT.COM LongFileNameTest.txt A:\\LONG.TXT > COM1\r\n"
                b"TYPE A:\\HELLO.TXT > COM1\r\n"
                b"ECHO EXFAT_DONE > COM1\r\n",
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

        log = td_path / "exfat.log"
        proc = subprocess.Popen(
            [
                "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
                "-m", "256", "-accel", "tcg", "-display", "none",
                "-boot", "a",
                "-serial", f"file:{log}",
                "-drive", f"file={test_img},format=raw,if=floppy",
                "-drive", f"id=stick,file={stick_img},format=raw,if=none,readonly=on",
                "-device", "pci-ohci,id=ohci",
                "-device", "usb-hub,bus=ohci.0,port=1",
                "-device", "usb-storage,bus=ohci.0,port=1.1,drive=stick",
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 60.0
            text = ""
            while time.time() < deadline:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "EXFAT_DONE" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        # Byte-exact verification of what DOS copied onto the boot floppy.
        copied = {}
        for name in ("HELLO.TXT", "BIG.BIN", "DOOM.TXT", "LONG.TXT"):
            out = td_path / ("copied_" + name)
            result = subprocess.run(
                ["mcopy", "-o", "-i", str(test_img), "::/" + name, str(out)],
                capture_output=True,
            )
            copied[name] = out.read_bytes() if result.returncode == 0 else None

        checks = [
            ("USB mass storage enumerated behind hub", "[MSC]" in text and "msc=1" in text),
            ("FSLOAD dynamically loaded EXFAT.JLM", "FSLOAD: loaded EXFAT.JLM (0xFD02)" in text),
            ("exFAT volume mounted with its label", 'EXFAT: label "EXFATTEST"' in text),
            ("root listing shows HELLO.TXT", "HELLO.TXT" in text),
            ("root listing shows BIG.BIN with its size", "BIG.BIN" in text and "10000 B" in text),
            ("root listing shows the long-named file", exfat_img.LONG_NAME in text),
            ("root listing shows GAMES as a directory", "GAMES <DIR>" in text),
            ("HELLO.TXT copied (27 bytes)", "EXFAT: Copied 27 bytes" in text),
            ("BIG.BIN copied (10000 bytes)", "EXFAT: Copied 10000 bytes" in text),
            ("HELLO.TXT byte-exact on the DOS floppy", copied["HELLO.TXT"] == exfat_img.HELLO_PAYLOAD),
            ("BIG.BIN byte-exact across its scattered chain", copied["BIG.BIN"] == exfat_img.BIG_PAYLOAD),
            ("subdirectory file byte-exact", copied["DOOM.TXT"] == exfat_img.DOOM_PAYLOAD),
            ("long-named file byte-exact", copied["LONG.TXT"] == exfat_img.LONG_PAYLOAD),
            ("HELLO.TXT content reached the serial log", "FERRIC-EXFAT-RO-PASS-2026" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[exfat] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
