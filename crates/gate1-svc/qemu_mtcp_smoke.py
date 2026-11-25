#!/usr/bin/env python3
"""Gate 2 step 2.3 acceptance: unmodified mTCP on the Ferric
packet driver in QEMU (emulated 82550, slirp user-net).

Boots the Connected profile (NETINIT installs the driver), then
runs mTCP's own DHCP.EXE and HTGET.EXE from a B: floppy: lease an
address from slirp's DHCP server, fetch a 1 MiB file over HTTP
from the host (10.0.2.2), and verify the downloaded copy's size
and SHA-256 afterwards with mtools.

The mTCP binaries are GPL and are not committed to this repo; the
smoke reads them from MTCP_BIN_DIR (default /work/out/mtcp-bin)
and SKIPs cleanly when they are absent.

The reference E100PKT.COM driver cannot serve as the QEMU
baseline for this test: it binds the emulated chip but fails its
own PORT self-test against QEMU's 82550 emulation ("CROM
signature is still 0"). It remains the parity reference on real
hardware, where its self-test passes.
"""
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

FILE_SIZE = 1024 * 1024
PORT = 8077
BIN_NAMES = ("DHCP.EXE", "HTGET.EXE")


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    bin_dir = Path(os.environ.get("MTCP_BIN_DIR", "/work/out/mtcp-bin"))
    if not all((bin_dir / n).exists() for n in BIN_NAMES):
        print("[gate2-mtcp] SKIP: mTCP binaries not present "
              f"(looked in {bin_dir})")
        return 0

    with tempfile.TemporaryDirectory(prefix="gate2-mtcp-") as td:
        td_path = Path(td)
        www = td_path / "www"
        www.mkdir()
        payload = bytes(range(256)) * (FILE_SIZE // 256)
        (www / "TESTFILE.BIN").write_bytes(payload)
        want_sha = hashlib.sha256(payload).hexdigest()
        server = subprocess.Popen(
            [sys.executable, "-m", "http.server", str(PORT),
             "--bind", "0.0.0.0", "--directory", str(www)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            # A: image - Connected profile + the mTCP block appended
            # after the MTEST status line (NETINIT has already run).
            test_img = td_path / "a.img"
            shutil.copy2(base_img, test_img)
            block = (
                b"\r\nSET MTCPCFG=B:\\MTCP.CFG\r\n"
                b"B:\\DHCP.EXE > COM1\r\n"
                b"B:\\HTGET.EXE -o B:\\TESTFILE.BIN "
                b"http://10.0.2.2:8077/TESTFILE.BIN > COM1\r\n"
                b"ECHO MTCP-DONE > COM1\r\n"
            )
            for filename, before, after in (
                ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=2,1"),
                ("AUTOEXEC.BAT", b"\r\nMTEST.COM STATUS\r\n",
                 b"\r\nMTEST.COM STATUS\r\n" + block),
            ):
                path = td_path / filename
                subprocess.run(
                    ["mcopy", "-o", "-i", str(test_img), "::/" + filename,
                     str(path)],
                    check=True,
                )
                data = path.read_bytes()
                if data.count(before) != 1:
                    raise ValueError(f"Expected one {before!r} in {filename}")
                path.write_bytes(data.replace(before, after))
                subprocess.run(
                    ["mcopy", "-o", "-i", str(test_img), str(path),
                     "::/" + filename],
                    check=True,
                )

            # B: floppy - the mTCP binaries and a minimal config.
            b_img = td_path / "b.img"
            subprocess.run(
                ["mformat", "-C", "-i", str(b_img), "-f", "1440", "::"],
                check=True,
            )
            cfg = td_path / "MTCP.CFG"
            cfg.write_bytes(
                b"PACKETINT 0x60\r\nHOSTNAME DYNABOOK\r\nMTU 1500\r\n")
            for name in BIN_NAMES:
                subprocess.run(
                    ["mcopy", "-o", "-i", str(b_img), str(bin_dir / name),
                     "::/" + name],
                    check=True,
                )
            subprocess.run(
                ["mcopy", "-o", "-i", str(b_img), str(cfg), "::/MTCP.CFG"],
                check=True,
            )

            log = td_path / "mtcp.log"
            proc = subprocess.Popen(
                [
                    "qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
                    "-m", "256", "-accel", "tcg", "-display", "none",
                    "-serial", f"file:{log}",
                    "-netdev", "user,id=n0",
                    "-device", "i82550,netdev=n0",
                    "-drive", f"file={test_img},format=raw,if=floppy",
                    "-drive", f"file={b_img},format=raw,if=floppy,index=1",
                ],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            try:
                deadline = time.time() + 300.0
                text = ""
                while time.time() < deadline:
                    if log.exists():
                        text = log.read_text(errors="replace")
                        if "MTCP-DONE" in text:
                            break
                    time.sleep(0.5)
            finally:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        finally:
            server.terminate()

        cfg_after = ""
        got_sha = None
        got_size = None
        try:
            cfg_path = td_path / "MTCP2.CFG"
            subprocess.run(
                ["mcopy", "-o", "-i", str(b_img), "::/MTCP.CFG",
                 str(cfg_path)],
                check=True, capture_output=True,
            )
            cfg_after = cfg_path.read_text(errors="replace")
        except subprocess.CalledProcessError:
            pass
        try:
            dl_path = td_path / "DL.BIN"
            subprocess.run(
                ["mcopy", "-o", "-i", str(b_img), "::/TESTFILE.BIN",
                 str(dl_path)],
                check=True, capture_output=True,
            )
            data = dl_path.read_bytes()
            got_size = len(data)
            got_sha = hashlib.sha256(data).hexdigest()
        except subprocess.CalledProcessError:
            pass

        checks = [
            ("mTCP block completed", "MTCP-DONE" in text),
            ("DHCP lease written (IPADDR 10.0.2.)",
             "IPADDR 10.0.2." in cfg_after),
            ("download size matches", got_size == FILE_SIZE),
            ("download SHA-256 matches", got_sha == want_sha),
        ]
        ok = True
        for name, passed in checks:
            print(f"[gate2-mtcp] {'PASS' if passed else 'FAIL'}: {name}")
            ok = ok and passed
        if not ok:
            print(text[-2000:])
            print(cfg_after)
        return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
