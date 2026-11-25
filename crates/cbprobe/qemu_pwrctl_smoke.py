"""Gate 5a QEMU smoke test for PWRCTL.COM.

Verifies under QEMU (Pentium III, TCG, Connected profile with JEMM386 + FERRIC.JLM):
  1. PWRCTL STATUS runs cleanly without hanging or crashing (APM + VBE/PM + idle).
  2. PWRCTL IDLE ON installs the INT 28h / INT 2Fh AX=1680h HLT TSR cleanly.
  3. PWRCTL STATUS detects idle=ON after TSR installation.
  4. PWRCTL IDLE OFF disables the TSR cleanly and PWRCTL STATUS reports idle=OFF.
  5. PWRCTL OFF and PWRCTL ON execute VBE/PM 1.0 display state calls cleanly.
"""

import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="gate5a-pwrctl-") as td:
        td_path = Path(td)
        test_img = td_path / "pwrctl.img"
        shutil.copy2(base_img, test_img)

        pwrctl_cmds = (
            b"MTEST.COM STATUS\r\n"
            b"PWRCTL.COM STATUS > COM1\r\n"
            b"PWRCTL.COM IDLE ON > COM1\r\n"
            b"PWRCTL.COM STATUS > COM1\r\n"
            b"PWRCTL.COM IDLE OFF > COM1\r\n"
            b"PWRCTL.COM STATUS > COM1\r\n"
            b"PWRCTL.COM OFF > COM1\r\n"
            b"PWRCTL.COM ON > COM1\r\n"
            b"ECHO PWRCTL_SMOKE_DONE > COM1\r\n"
        )

        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
            ("AUTOEXEC.BAT", b"MTEST.COM STATUS\r\n", pwrctl_cmds),
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

        log = td_path / "pwrctl.log"
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
                    if "PWRCTL_SMOKE_DONE" in text:
                        break
                time.sleep(0.25)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

        checks = [
            ("PWRCTL banner printed", "Ferric DOS Power Control (PWRCTL)" in text),
            ("APM 1.2 detected under QEMU", "PWRCTL: apm=1 ver=1.2 " in text),
            ("Initial status reports idle=UNLOADED", "idle=UNLOADED" in text),
            ("IDLE ON installs TSR", "PWRCTL: idle=ON" in text),
            ("Status after IDLE ON reports idle=ON", "vbepm=1 caps=0x0F idle=ON" in text),
            ("IDLE OFF disables TSR", "PWRCTL: idle=OFF" in text),
            ("Status after IDLE OFF reports idle=OFF", "vbepm=1 caps=0x0F idle=OFF" in text),
            ("VBE/PM OFF sets state 0x04 cleanly", "VBE/PM display state set: 0x04" in text),
            ("VBE/PM ON restores state 0x00 cleanly", "VBE/PM display state set: 0x00" in text),
            ("Smoke sequence completed without hang", "PWRCTL_SMOKE_DONE" in text),
        ]
        ok = True
        for name, passed in checks:
            print(f"[gate5a-pwrctl] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-1600:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
