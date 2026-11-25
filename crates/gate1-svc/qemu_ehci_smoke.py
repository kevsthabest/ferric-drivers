"""QEMU TCG smoke test for Stage H Steps 2-3: EHCPROBE.COM recon
(verb 0x0036) and bring-up (verb 0x0037) against QEMU's usb-ehci
device with a high-speed usb-storage stick attached.

Assertions stay honest about what QEMU's model actually reports: the
port count is asserted >= 1 (not a hard-coded number) and the raw
HCSPARAMS/HCCPARAMS lines from the recon report are echoed into the
PASS output so a human can see the model's real values.
"""

import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import make_exfat_image as exfat_img


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
            "-device", "usb-ehci,id=ehci",
            "-device", "usb-storage,bus=ehci.0,drive=stick",
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
                    print(f"[ehci] qemu stderr ({tag}): {err.strip()}")
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
    with tempfile.TemporaryDirectory(prefix="ehci-") as td:
        td_path = Path(td)

        stick = td_path / "stick.img"
        exfat_img.build_image(stick)

        recon_script = (
            b"MTEST.COM STATUS\r\n"
            b"EHCPROBE.COM > COM1\r\n"
            b"ECHO EHCI_RECON_DONE > COM1\r\n"
        )
        recon_log = run_case(base_img, td_path, "recon", stick, recon_script, "EHCI_RECON_DONE")

        bringup_script = (
            b"MTEST.COM STATUS\r\n"
            b"EHCPROBE.COM /B > COM1\r\n"
            b"FSLOAD.COM > COM1\r\n"
            b"EXFAT.COM > COM1\r\n"
            b"EXFAT.COM HELLO.TXT A:\\HELLO.TXT > COM1\r\n"
            b"TYPE A:\\HELLO.TXT > COM1\r\n"
            b"EHCPROBE.COM /U > COM1\r\n"
            b"ECHO EHCI_BRINGUP_DONE > COM1\r\n"
        )
        bringup_log = run_case(base_img, td_path, "bringup", stick, bringup_script, "EHCI_BRINGUP_DONE")

        ports_match = re.search(r"EHCI: hcsparams=0x[0-9A-Fa-f]+ ports=([0-9]+)", recon_log)
        n_ports = int(ports_match.group(1)) if ports_match else 0
        hcs_line = next((ln for ln in recon_log.splitlines() if "hcsparams=" in ln), "")
        hcc_line = next((ln for ln in recon_log.splitlines() if "hccparams=" in ln), "")

        checks = [
            ("recon: controller found", "EHCI: " in recon_log and "EHCI: none found" not in recon_log),
            ("recon: HCIVERSION is 0x0100", re.search(r"hciversion=0x0100", recon_log) is not None),
            ("recon: at least one port", n_ports >= 1),
            ("bring-up: controller running (not halted, async schedule on)",
             re.search(r"EHCI: usbsts=0x[0-9A-Fa-f]+ halted=0 async=1", bringup_log) is not None),
            ("bring-up: CONFIGFLAG claimed (routing = EHCI)", "EHCI: configflag=1" in bringup_log),
            ("bring-up: stick port classified high-speed, owner ehci",
             re.search(r"EHCI: port[0-9]+ connect=1 enabled=1 line=\S+ speed=high owner=ehci", bringup_log) is not None),
            ("step 4: EHCI MSC enumerated and bound (EXFAT)",
             re.search(r"EHCI: msc port=[0-9]+ addr=[0-9]+ sectors=[0-9]+ fs=EXFAT", bringup_log) is not None),
            ("step 4: FSLOAD loaded EXFAT.JLM over EHCI MSC", "EXFAT.JLM" in bringup_log),
            ("step 4: EXFAT.COM mounted volume and copied HELLO.TXT byte-exact over EHCI",
             "EXFATTEST" in bringup_log and "EXFAT: Copied 27 bytes" in bringup_log and "FERRIC-EXFAT-RO-PASS-2026" in bringup_log),
            ("step 5: EHCPROBE /U shut down EHCI and restored CONFIGFLAG=0",
             "EHCPROBE: EHCI shutdown (CONFIGFLAG=0)" in bringup_log),
        ]
        ok = True
        for name, passed in checks:
            print(f"[ehci] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if hcs_line:
            print(f"[ehci] model values: {hcs_line.strip()}")
        if hcc_line:
            print(f"[ehci] model values: {hcc_line.strip()}")
        if not ok:
            print("--- RECON LOG ---")
            print(recon_log[-1500:])
            print("--- BRINGUP LOG ---")
            print(bringup_log[-1500:])
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
