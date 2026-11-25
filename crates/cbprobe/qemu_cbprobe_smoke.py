"""Gate 4b step 4b.1 QEMU smoke for CBPROBE.COM.

QEMU has no ToPIC95, so this proves the absence leg: the probe runs
under DOS, reads PCI configuration space for both bridge functions,
reports present=0 for each, prints the bridges=0 summary, and exits
without disturbing the Connected profile boot. The presence leg is
the DynaBook capture (docs/hardware/), which needs the real card
and bridges.
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
    with tempfile.TemporaryDirectory(prefix="gate4b-cbprobe-") as td:
        td_path = Path(td)
        test_img = td_path / "cbprobe.img"
        shutil.copy2(base_img, test_img)

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            (
                "AUTOEXEC.BAT",
                b"MTEST.COM STATUS\r\n",
                b"MTEST.COM STATUS\r\nCBPROBE.COM > COM1\r\nCBINIT.COM > COM1\r\nWPROBE.COM > COM1\r\nWSCAN.COM > COM1\r\nWCONN.COM TestSSID > COM1\r\n",
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

        log = td_path / "cbprobe.log"
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
            deadline = time.time() + 60.0
            text = ""
            while time.time() < deadline:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "WCONN: status=" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("function 0 absent under QEMU", "CBPROBE: fn=0 present=0" in text),
            ("function 1 absent under QEMU", "CBPROBE: fn=1 present=0" in text),
            ("summary reports zero bridges", "CBPROBE: bridges=0" in text),
            ("CBINIT slot 0 absent under QEMU", "CBINIT: slot=0 present=0" in text),
            ("CBINIT slot 1 absent under QEMU", "CBINIT: slot=1 present=0" in text),
            ("CBINIT summary reports zero bridges/cards", "CBINIT: bridges=0 cards=0" in text),
            ("WPROBE reports absent under QEMU", "WPROBE: present=0 stage=0" in text),
            ("WSCAN reports absent under QEMU", "WSCAN: present=0 stage=0 aps=0" in text),
            ("WCONN reports no card cleanly under QEMU", "WCONN: status=1 (FAILED)" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[gate4b-cbprobe] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1200:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
