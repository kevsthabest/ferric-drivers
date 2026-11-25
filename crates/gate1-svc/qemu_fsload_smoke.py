"""QEMU TCG smoke test for Stage F FSLOAD.COM lifecycle (load -> use ->
unload -> fail-closed while unloaded -> reload -> use) across both
standalone filesystem JLMs: EXFAT.JLM (0xFD02) and NTFS.JLM (0xFD03).
"""

import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import make_exfat_image as exfat_img
import make_ntfs_image as ntfs_img


def run_case(base_img: Path, td_path: Path, tag: str, stick_img: Path, autoexec_add: bytes, done: str) -> str:
    test_img = td_path / f"{tag}_floppy.img"
    shutil.copy2(base_img, test_img)

    for filename, before, after in (
        ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
        ("AUTOEXEC.BAT", b"MTEST.COM STATUS\r\n", autoexec_add),
    ):
        path = td_path / f"{tag}_{filename}"
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

    log = td_path / f"{tag}.log"
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
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.time() + 60.0
        text = ""
        while time.time() < deadline:
            if proc.poll() is not None:
                err = proc.stderr.read().decode(errors="replace") if proc.stderr else ""
                if err:
                    print(f"[fsload] qemu stderr ({tag}): {err.strip()}")
                break
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
    with tempfile.TemporaryDirectory(prefix="fsload-") as td:
        td_path = Path(td)

        exfat_stick = td_path / "exfat.img"
        exfat_img.build_image(exfat_stick)
        exfat_script = (
            b"MTEST.COM STATUS\r\n"
            b"USBPROBE.COM > COM1\r\n"
            b"FSLOAD.COM > COM1\r\n"
            b"EXFAT.COM > COM1\r\n"
            b"XREDIR.COM E: > COM1\r\n"
            b"DIR E:\\ > COM1\r\n"
            b"FSLOAD.COM /U > COM1\r\n"
            b"ECHO AFTER_EXFAT_UNLOAD > COM1\r\n"
            b"DIR A:\\ > COM1\r\n"
            b"EXFAT.COM > COM1\r\n"
            b"FSLOAD.COM > COM1\r\n"
            b"ECHO AFTER_EXFAT_RELOAD > COM1\r\n"
            b"EXFAT.COM > COM1\r\n"
            b"ECHO FSLOAD_EXFAT_DONE > COM1\r\n"
        )
        exfat_log = run_case(base_img, td_path, "exfat", exfat_stick, exfat_script, "FSLOAD_EXFAT_DONE")
        ex_p1, _, ex_rest = exfat_log.partition("AFTER_EXFAT_UNLOAD")
        ex_p2, _, ex_p3 = ex_rest.partition("AFTER_EXFAT_RELOAD")

        ntfs_stick = td_path / "ntfs.img"
        ntfs_img.build_image(ntfs_stick)
        ntfs_script = (
            b"MTEST.COM STATUS\r\n"
            b"USBPROBE.COM > COM1\r\n"
            b"FSLOAD.COM > COM1\r\n"
            b"NTFS.COM > COM1\r\n"
            b"FSLOAD.COM /U > COM1\r\n"
            b"ECHO AFTER_NTFS_UNLOAD > COM1\r\n"
            b"NTFS.COM > COM1\r\n"
            b"FSLOAD.COM > COM1\r\n"
            b"ECHO AFTER_NTFS_RELOAD > COM1\r\n"
            b"NTFS.COM > COM1\r\n"
            b"ECHO FSLOAD_NTFS_DONE > COM1\r\n"
        )
        ntfs_log = run_case(base_img, td_path, "ntfs", ntfs_stick, ntfs_script, "FSLOAD_NTFS_DONE")
        nt_p1, _, nt_rest = ntfs_log.partition("AFTER_NTFS_UNLOAD")
        nt_p2, _, nt_p3 = nt_rest.partition("AFTER_NTFS_RELOAD")

        checks = [
            ("exFAT: USBPROBE detected EXFAT", "USBPROBE: fs=EXFAT" in ex_p1),
            ("exFAT: FSLOAD loaded EXFAT.JLM (0xFD02)", "FSLOAD: loaded EXFAT.JLM (0xFD02)" in ex_p1),
            ("exFAT: EXFAT.COM listed volume before unload", 'EXFAT: label "EXFATTEST"' in ex_p1),
            ("exFAT: XREDIR mounted E: before unload", "XREDIR: exFAT volume mounted as E:" in ex_p1),
            ("exFAT: FSLOAD /U detached XREDIR before unloading", "FSLOAD: detached XREDIR drive" in ex_p1),
            ("exFAT: FSLOAD /U unloaded EXFAT.JLM (0xFD02)", "FSLOAD: unloaded EXFAT.JLM (0xFD02)" in ex_p1),
            ("exFAT: DOS DIR A:\\ works after unload (no Exception 06)", "FERRIC" in ex_p2),
            ("exFAT: EXFAT.COM fails closed while unloaded", "EXFAT.JLM (0xFD02) not loaded" in ex_p2),
            ("exFAT: FSLOAD reloaded EXFAT.JLM (0xFD02)", "FSLOAD: loaded EXFAT.JLM (0xFD02)" in ex_p2),
            ("exFAT: EXFAT.COM listed volume after reload", 'EXFAT: label "EXFATTEST"' in ex_p3),
            ("NTFS: USBPROBE detected NTFS", "USBPROBE: fs=NTFS" in nt_p1),
            ("NTFS: FSLOAD loaded NTFS.JLM (0xFD03)", "FSLOAD: loaded NTFS.JLM (0xFD03)" in nt_p1),
            ("NTFS: NTFS.COM listed volume before unload", 'NTFS: label "NTFSTEST"' in nt_p1),
            ("NTFS: FSLOAD /U unloaded NTFS.JLM (0xFD03)", "FSLOAD: unloaded NTFS.JLM (0xFD03)" in nt_p1),
            ("NTFS: NTFS.COM fails closed while unloaded", "NTFS.JLM (0xFD03) not loaded" in nt_p2),
            ("NTFS: FSLOAD reloaded NTFS.JLM (0xFD03)", "FSLOAD: loaded NTFS.JLM (0xFD03)" in nt_p2),
            ("NTFS: NTFS.COM listed volume after reload", 'NTFS: label "NTFSTEST"' in nt_p3),
        ]
        ok = True
        for name, passed in checks:
            print(f"[fsload] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print("--- EXFAT LOG ---")
            print(exfat_log[-1500:])
            print("--- NTFS LOG ---")
            print(ntfs_log[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
