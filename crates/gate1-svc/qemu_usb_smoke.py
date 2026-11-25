"""Gate 2.4 QEMU TCG smoke test for Rust OHCI USB + External Hub + HID enumeration.

Attaches a QEMU `pci-ohci` controller (`106B:003F`) with a `usb-hub` on root port 1
and a `usb-mouse` + `usb-kbd` behind the hub (ports 1.1 and 1.2), boots Profile 2
(Connected), runs `USBPROBE.COM`, and asserts end-to-end enumeration through both
the OHCI root hub and the external USB hub.
"""

import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="gate2-usb-") as td:
        td_path = Path(td)
        test_img = td_path / "usb.img"
        shutil.copy2(base_img, test_img)

        # Create an 8 MiB FAT16 virtual USB flash drive containing HELLO.TXT
        stick_img = td_path / "stick.img"
        subprocess.run(
            ["dd", "if=/dev/zero", f"of={stick_img}", "bs=1M", "count=8", "status=none"],
            check=True,
        )
        subprocess.run(
            ["mkfs.fat", "-F", "16", "-s", "2", "-n", "USBTEST", str(stick_img)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        hello = td_path / "HELLO.TXT"
        hello.write_bytes(b"FERRIC-USB-RO-PASS-2026\r\n")
        subprocess.run(
            ["mcopy", "-i", str(stick_img), str(hello), "::/HELLO.TXT"],
            check=True,
        )

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"MTEST.COM STATUS\r\n",
                b"MTEST.COM STATUS\r\nUSBPROBE.COM > COM1\r\nUSBCP.COM > COM1\r\nUSBCP.COM HELLO.TXT A:\\COPIED.TXT > COM1\r\nTYPE A:\\COPIED.TXT > COM1\r\nDIR C:\\ > COM1\r\nTYPE C:\\HELLO.TXT > COM1\r\nECHO USB_DONE > COM1\r\n",
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

        log = td_path / "usb.log"
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
                "-device", "usb-mouse,bus=ohci.0,port=1.1",
                "-device", "usb-kbd,bus=ohci.0,port=1.2",
                "-device", "usb-storage,bus=ohci.0,port=1.3,drive=stick",
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 60.0
            text = ""
            while time.time() < deadline:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "USB_DONE" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("OHCI controller initialized (stage=5, 106B:003F)", "USBPROBE: present=1 stage=5 id=106B:003F" in text),
            ("4 USB devices enumerated (hub + mouse + kbd + msc)", "devs=4 hubs=1" in text),
            ("external USB hub enumerated", "[HUB ports=" in text),
            ("USB mouse enumerated behind hub", "par=1:1" in text and "if=03/01/02" in text and "[MOUSE]" in text),
            ("USB keyboard enumerated behind hub", "par=1:2" in text and "if=03/01/01" in text and "[KBD]" in text),
            ("USB mass storage enumerated behind hub", "par=1:3" in text and "[MSC]" in text),
            ("live USB HID+MSC resident", "USBPROBE: active (mouse=1 kbd=1 msc=1) - resident" in text),
            ("USBCP lists HELLO.TXT on FAT16 USB drive", "FAT16" in text and "HELLO.TXT" in text),
            ("USBCP copies HELLO.TXT byte-for-byte", "USBCP: Copied 25 bytes" in text and "FERRIC-USB-RO-PASS-2026" in text),
            ("USBDRV.SYS mounts USB stick as native DOS drive letter", "USBTEST" in text and text.count("FERRIC-USB-RO-PASS-2026") >= 2),
        ]
        ok = True
        for name, passed in checks:
            print(f"[gate2-usb] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
