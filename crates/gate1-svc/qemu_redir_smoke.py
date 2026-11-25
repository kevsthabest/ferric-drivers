"""QEMU TCG smoke test for Stage C: the exFAT volume on a real DOS
drive letter through the INT 2Fh redirector (XREDIR.COM + JLM verbs
0x0030-0x0031).

Boots Profile 2 (Connected) with the generated exFAT stick on the
emulated OHCI hub, installs XREDIR for drive E:, and drives the volume
through plain DOS commands: DIR, TYPE, COPY in both directions, CD
into a subdirectory. Asserts the listings, the volume label, byte-exact
copies onto the boot floppy, that a write into a nonexistent
directory on E: fails cleanly, and that the stick image is
bit-identical afterwards. (The stick stays attached readonly=on in
this smoke; the writable path has its own smoke,
qemu_redir_write_smoke.py.) A second boot without the stick asserts
XREDIR refuses to install instead of hanging.
"""

import hashlib
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import make_exfat_image as exfat_img

AUTOEXEC_ADD = (
    b"MTEST.COM STATUS\r\n"
    b"USBPROBE.COM > COM1\r\n"
    b"FSLOAD.COM > COM1\r\n"
    b"XREDIR.COM E: > COM1\r\n"
    b"DIR E:\\ > COM1\r\n"
    b"TYPE E:\\HELLO.TXT > COM1\r\n"
    b"COPY E:\\BIG.BIN A:\\BIG.BIN > COM1\r\n"
    b"COPY E:\\HELLO.TXT A:\\HELLO.TXT > COM1\r\n"
    b"CD E:\\GAMES > COM1\r\n"
    b"DIR E: > COM1\r\n"
    b"COPY E:\\GAMES\\DOOM.TXT A:\\DOOM.TXT > COM1\r\n"
    b"COPY A:\\HELLO.TXT E:\\NODIR\\X.TXT > COM1\r\n"
    b"ECHO XREDIR_DONE > COM1\r\n"
)


def patch_boot_files(td_path: Path, test_img: Path) -> None:
    for filename, before, after in (
        ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
        ("AUTOEXEC.BAT", b"MTEST.COM STATUS\r\n", AUTOEXEC_ADD),
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


def boot(td_path: Path, test_img: Path, stick_img: Path | None) -> str:
    log = td_path / ("redir.log" if stick_img else "redir-absent.log")
    cmd = [
        "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
        "-m", "256", "-accel", "tcg", "-display", "none",
        "-boot", "a",
        "-serial", f"file:{log}",
        "-drive", f"file={test_img},format=raw,if=floppy",
    ]
    if stick_img is not None:
        cmd += [
            "-drive", f"id=stick,file={stick_img},format=raw,if=none,readonly=on",
            "-device", "pci-ohci,id=ohci",
            "-device", "usb-hub,bus=ohci.0,port=1",
            "-device", "usb-storage,bus=ohci.0,port=1.1,drive=stick",
        ]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.time() + 90.0
        text = ""
        while time.time() < deadline:
            if log.exists():
                text = log.read_text(errors="replace")
                if "XREDIR_DONE" in text:
                    break
            time.sleep(0.25)
        return text
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="redir-") as td:
        td_path = Path(td)
        test_img = td_path / "usb.img"
        shutil.copy2(base_img, test_img)
        patch_boot_files(td_path, test_img)

        stick_img = td_path / "stick.img"
        exfat_img.build_image(stick_img)
        stick_hash_before = hashlib.sha256(stick_img.read_bytes()).hexdigest()

        text = boot(td_path, test_img, stick_img)
        stick_hash_after = hashlib.sha256(stick_img.read_bytes()).hexdigest()

        copied = {}
        for name in ("BIG.BIN", "DOOM.TXT", "HELLO.TXT"):
            out = td_path / ("copied_" + name)
            result = subprocess.run(
                ["mcopy", "-o", "-i", str(test_img), "::/" + name, str(out)],
                capture_output=True,
            )
            copied[name] = out.read_bytes() if result.returncode == 0 else None

        checks = [
            ("XREDIR installed drive E:", "XREDIR: exFAT volume mounted as E:" in text),
            ("DIR shows the volume label", "EXFATTEST" in text),
            ("DIR E:\\ lists HELLO.TXT", "HELLO" in text and "TXT" in text),
            ("DIR E:\\ lists BIG.BIN", "BIG" in text and "BIN" in text),
            ("DIR E:\\ lists the GAMES directory", "GAMES" in text),
            (
                "long name not mangled into the listing",
                exfat_img.LONG_NAME not in text and "LONGFILE" not in text,
            ),
            ("TYPE printed the payload", "FERRIC-EXFAT-RO-PASS-2026" in text),
            ("BIG.BIN byte-exact through the drive letter", copied["BIG.BIN"] == exfat_img.BIG_PAYLOAD),
            ("HELLO.TXT byte-exact through the drive letter", copied["HELLO.TXT"] == exfat_img.HELLO_PAYLOAD),
            ("CD E:\\GAMES then DIR E: lists DOOM.TXT", "DOOM" in text),
            ("subdirectory file byte-exact", copied["DOOM.TXT"] == exfat_img.DOOM_PAYLOAD),
            (
                "write into a nonexistent directory failed and left the stick bit-identical",
                stick_hash_before == stick_hash_after,
            ),
            ("guest reached the end of the script", "XREDIR_DONE" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[redir] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])

        # Second boot, no stick: XREDIR must refuse, not hang.
        test_img2 = td_path / "usb2.img"
        shutil.copy2(base_img, test_img2)
        patch_boot_files(td_path, test_img2)
        text2 = boot(td_path, test_img2, None)
        checks2 = [
            ("stick absent: XREDIR refuses to install", "XREDIR: No exFAT volume mounted" in text2),
            ("stick absent: no drive claimed", "mounted as" not in text2),
            ("stick absent: guest did not hang", "XREDIR_DONE" in text2),
        ]
        for name, passed in checks2:
            print(f"[redir] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not all(p for _, p in checks2):
            print(text2[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
