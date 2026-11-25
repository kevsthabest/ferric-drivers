"""QEMU TCG smoke test for Stage D: writing to the exFAT volume
through the drive letter (XREDIR.COM + the redirector's write
family in the JLM).

Same boot as the Stage C read smoke, but the stick is attached
READ-WRITE (only in this smoke — the read smokes keep readonly=on).
Through plain DOS commands on E:: COPY a text payload in, DIR shows
it with the right size, TYPE reads it back, COPY a binary payload in
and back out byte-exact, write a second file into E:\\GAMES, DELETE
the first file, and DIR confirms its absence. Then the guest REBOOTS
against the same stick image and confirms the second file persisted
and the binary still reads byte-exact — proving the on-disk state,
not a cache.
"""

import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import make_exfat_image as exfat_img

IN1_TXT = b"Ferric write smoke line\r\n" * 40  # 1,000 bytes
IN2_BIN = bytes(((i * 13 + 7) & 0xFF) for i in range(5000))

AUTOEXEC_1 = (
    b"MTEST.COM STATUS\r\n"
    b"USBPROBE.COM > COM1\r\n"
    b"FSLOAD.COM > COM1\r\n"
    b"XREDIR.COM E: > COM1\r\n"
    b"COPY A:\\IN1.TXT E:\\IN1.TXT > COM1\r\n"
    b"DIR E:\\ > COM1\r\n"
    b"TYPE E:\\IN1.TXT > COM1\r\n"
    b"COPY A:\\IN2.BIN E:\\IN2.BIN > COM1\r\n"
    b"COPY E:\\IN2.BIN A:\\BACK2.BIN > COM1\r\n"
    b"COPY A:\\IN1.TXT E:\\GAMES\\PAY2.TXT > COM1\r\n"
    b"TYPE E:\\GAMES\\PAY2.TXT > COM1\r\n"
    b"DEL E:\\IN1.TXT > COM1\r\n"
    b"IF NOT EXIST E:\\IN1.TXT ECHO IN1_DELETED > COM1\r\n"
    b"IF EXIST E:\\IN2.BIN ECHO IN2_STILL_THERE > COM1\r\n"
    b"ECHO AFTER_DEL > COM1\r\n"
    b"DIR E:\\ > COM1\r\n"
    b"ECHO REDIRW_DONE > COM1\r\n"
)

AUTOEXEC_2 = (
    b"MTEST.COM STATUS\r\n"
    b"USBPROBE.COM > COM1\r\n"
    b"FSLOAD.COM > COM1\r\n"
    b"XREDIR.COM E: > COM1\r\n"
    b"IF NOT EXIST E:\\IN1.TXT ECHO IN1_GONE_AFTER_REBOOT > COM1\r\n"
    b"TYPE E:\\GAMES\\PAY2.TXT > COM1\r\n"
    b"COPY E:\\IN2.BIN A:\\BACK2B.BIN > COM1\r\n"
    b"ECHO REDIRW2_DONE > COM1\r\n"
)


def patch_boot_files(td_path: Path, test_img: Path, autoexec_add: bytes) -> None:
    for filename, before, after in (
        ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
        ("AUTOEXEC.BAT", b"MTEST.COM STATUS\r\n", autoexec_add),
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


def add_payloads(td_path: Path, test_img: Path) -> None:
    for name, payload in (("IN1.TXT", IN1_TXT), ("IN2.BIN", IN2_BIN)):
        path = td_path / name
        path.write_bytes(payload)
        subprocess.run(
            ["mcopy", "-o", "-i", str(test_img), str(path), "::/" + name],
            check=True,
        )


def extract(test_img: Path, name: str, td_path: Path) -> bytes | None:
    out = td_path / ("copied_" + name)
    result = subprocess.run(
        ["mcopy", "-o", "-i", str(test_img), "::/" + name, str(out)],
        capture_output=True,
    )
    return out.read_bytes() if result.returncode == 0 else None


def boot(td_path: Path, test_img: Path, stick_img: Path, done: str, log_name: str) -> str:
    log = td_path / log_name
    cmd = [
        "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
        "-m", "256", "-accel", "tcg", "-display", "none",
        "-boot", "a",
        "-serial", f"file:{log}",
        # No readonly=on: this smoke is the one place the stick is
        # attached writable, because writing is what it tests.
        "-drive", f"id=stick,file={stick_img},format=raw,if=none",
        "-device", "pci-ohci,id=ohci",
        "-device", "usb-hub,bus=ohci.0,port=1",
        "-device", "usb-storage,bus=ohci.0,port=1.1,drive=stick",
        "-drive", f"file={test_img},format=raw,if=floppy",
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.time() + 90.0
        text = ""
        while time.time() < deadline:
            if log.exists():
                text = log.read_text(errors="replace")
                if done in text:
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
    with tempfile.TemporaryDirectory(prefix="redirw-") as td:
        td_path = Path(td)
        stick_img = td_path / "stick.img"
        exfat_img.build_image(stick_img)

        # Phase 1: write, read back, delete — all through E:.
        test_img = td_path / "usb.img"
        shutil.copy2(base_img, test_img)
        patch_boot_files(td_path, test_img, AUTOEXEC_1)
        add_payloads(td_path, test_img)
        text = boot(td_path, test_img, stick_img, "REDIRW_DONE", "redirw.log")

        back2 = extract(test_img, "BACK2.BIN", td_path)
        head, _, tail = text.partition("AFTER_DEL")
        checks = [
            ("XREDIR installed drive E:", "XREDIR: exFAT volume mounted as E:" in text),
            (
                "DIR lists IN1.TXT with its 1,000-byte size",
                "IN1" in head and ("1,000" in head or "1000" in head),
            ),
            (
                # 40 lines from TYPE IN1.TXT and 40 from TYPE
                # GAMES\PAY2.TXT: past 40, both TYPEs produced output.
                "TYPE read both written files back",
                text.count("Ferric write smoke line") >= 41,
            ),
            ("IN2.BIN copied back byte-exact", back2 == IN2_BIN),
            ("DEL removed IN1.TXT", "IN1_DELETED" in text),
            ("IN2.BIN survived the delete", "IN2_STILL_THERE" in text),
            (
                "post-delete DIR: IN1 gone, IN2 listed",
                "IN1" not in tail and "IN2" in tail,
            ),
            ("guest reached the end of the script", "REDIRW_DONE" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[redirw] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])

        # Phase 2: reboot against the same (now mutated) stick image.
        # Everything asserted here can only be true if phase 1's
        # writes actually reached the medium.
        test_img2 = td_path / "usb2.img"
        shutil.copy2(base_img, test_img2)
        patch_boot_files(td_path, test_img2, AUTOEXEC_2)
        text2 = boot(td_path, test_img2, stick_img, "REDIRW2_DONE", "redirw2.log")

        back2b = extract(test_img2, "BACK2B.BIN", td_path)
        checks2 = [
            ("after reboot: IN1.TXT still deleted", "IN1_GONE_AFTER_REBOOT" in text2),
            (
                "after reboot: GAMES\\PAY2.TXT persisted and reads back",
                text2.count("Ferric write smoke line") >= 40,
            ),
            ("after reboot: IN2.BIN still byte-exact", back2b == IN2_BIN),
            ("after reboot: guest reached the end", "REDIRW2_DONE" in text2),
        ]
        for name, passed in checks2:
            print(f"[redirw] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not all(p for _, p in checks2):
            print(text2[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
