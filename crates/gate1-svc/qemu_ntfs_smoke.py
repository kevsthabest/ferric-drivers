"""QEMU TCG smoke test for the NTFS read path (NTFS.COM + JLM verbs
0x0032-0x0034 over the OHCI USB mass-storage stack).

Builds a 16 MiB NTFS stick image with scripts/make_ntfs_image.py
(4 KiB clusters; BIG.BIN split by a sparse run), boots Profile 2
(Connected) with the stick on the emulated OHCI hub, runs USBPROBE +
NTFS.COM, and asserts the directory listing and byte-exact copies of
a root resident file, the sparse-split non-resident file, a two-
namespace long-named file, and a subdirectory file.
"""

import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import make_ntfs_image as ntfs_img


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="ntfs-") as td:
        td_path = Path(td)
        test_img = td_path / "usb.img"
        shutil.copy2(base_img, test_img)

        stick_img = td_path / "stick.img"
        ntfs_img.build_image(stick_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"MTEST.COM STATUS\r\n",
                b"MTEST.COM STATUS\r\n"
                b"USBPROBE.COM > COM1\r\n"
                b"FSLOAD.COM > COM1\r\n"
                b"NTFS.COM > COM1\r\n"
                b"NTFS.COM HELLO.TXT A:\\HELLO.TXT > COM1\r\n"
                b"NTFS.COM BIG.BIN A:\\BIG.BIN > COM1\r\n"
                b"NTFS.COM GAMES\\DOOM.TXT A:\\DOOM.TXT > COM1\r\n"
                b"NTFS.COM LongFileNameTest.txt A:\\LONG.TXT > COM1\r\n"
                b"TYPE A:\\HELLO.TXT > COM1\r\n"
                b"ECHO NTFS_DONE > COM1\r\n",
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

        log = td_path / "ntfs.log"
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
                    if "NTFS_DONE" in text:
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

        hello_len = len(ntfs_img.HELLO_PAYLOAD)
        big_len = len(ntfs_img.BIG_PAYLOAD)
        checks = [
            ("USB mass storage enumerated behind hub", "[MSC]" in text and "msc=1" in text),
            ("USBPROBE reports detected NTFS filesystem", "USBPROBE: fs=NTFS" in text),
            ("FSLOAD loads standalone NTFS.JLM (0xFD03)", "FSLOAD: loaded NTFS.JLM (0xFD03)" in text),
            ("NTFS volume mounted with its label", 'NTFS: label "NTFSTEST"' in text),
            ("volume serial reported", f"serial {ntfs_img.SERIAL}" in text),
            ("free clusters reported", f"free {ntfs_img.FREE_CLUSTERS} clusters" in text),
            ("root listing shows HELLO.TXT", "HELLO.TXT" in text),
            ("root listing shows BIG.BIN with its size", "BIG.BIN" in text and f"{big_len} B" in text),
            ("root listing shows the long-named file", ntfs_img.LONG_NAME in text),
            ("root listing shows GAMES as a directory", "GAMES <DIR>" in text),
            (f"HELLO.TXT copied ({hello_len} bytes)", f"NTFS: Copied {hello_len} bytes" in text),
            (f"BIG.BIN copied ({big_len} bytes)", f"NTFS: Copied {big_len} bytes" in text),
            ("HELLO.TXT byte-exact on the DOS floppy", copied["HELLO.TXT"] == ntfs_img.HELLO_PAYLOAD),
            ("BIG.BIN byte-exact across its sparse gap", copied["BIG.BIN"] == ntfs_img.BIG_PAYLOAD),
            ("subdirectory file byte-exact", copied["DOOM.TXT"] == ntfs_img.DOOM_PAYLOAD),
            ("long-named file byte-exact", copied["LONG.TXT"] == ntfs_img.LONG_PAYLOAD),
            ("HELLO.TXT content reached the serial log", "FERRIC-NTFS-RO-PASS-2026" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[ntfs] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
