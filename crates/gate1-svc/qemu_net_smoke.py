#!/usr/bin/env python3
"""Boot the Connected profile in QEMU with an emulated Intel 82550
(-device i82550, user-net/slirp) and verify Gate 2 step 2.3 end to
end: NETINIT installs the resident packet driver, and NTEST finds it
by its "PKT DRVR" signature, reads the QEMU station address
(52:54:00:..) through get_address, registers a receiver, sends an ARP
request for the slirp gateway (10.0.2.2), and receives the reply
through the two-phase upcall.

QEMU's 82550 presents device ID 8086:1209 (the DynaBook's silicon is
8086:1229); the driver binds both.
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
    with tempfile.TemporaryDirectory(prefix="gate2-net-") as td:
        td_path = Path(td)
        test_img = td_path / "net.img"
        shutil.copy2(base_img, test_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"\r\nMTEST.COM STATUS\r\n",
                b"\r\nMTEST.COM STATUS\r\nNTEST.COM > COM1\r\n",
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

        log = td_path / "net.log"
        proc = subprocess.Popen(
            [
                "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
                "-m", "256", "-accel", "tcg", "-display", "none",
                "-serial", f"file:{log}",
                "-netdev", "user,id=n0",
                "-device", "i82550,netdev=n0",
                "-drive", f"file={test_img},format=raw,if=floppy",
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 90.0
            text = ""
            while time.time() < deadline:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "NTEST:" in text and ("PASS" in text or "FAIL" in text):
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("driver found by signature", "NTEST: found=1" in text),
            ("QEMU station address read", "NTEST: mac=52-54-00" in text),
            ("ARP reply via upcall", "arp=1" in text),
            ("NTEST verdict", "NTEST: PASS" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[gate2-net] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1600:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
