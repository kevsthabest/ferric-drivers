#!/usr/bin/env python3
"""Exercise standalone Ethernet without FERRIC.JLM on a disposable FAT12 image."""
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

def screen(sock, dest):
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(2)
            s.connect(str(sock))
            s.sendall(f'pmemsave 0xb8000 4000 "{dest}"\n'.encode())
            time.sleep(0.25)
            if dest.exists():
                raw = dest.read_bytes()
                return "\n".join(raw[i:i+160:2].decode("cp437", "replace").rstrip()
                                 for i in range(0, min(len(raw), 4000), 160))
    except OSError:
        pass
    return "(screen unavailable)"

def run(base, output, name, commands, nic, checks):
    with tempfile.TemporaryDirectory(prefix="eth-qemu-") as work:
        work = Path(work)
        image, log, mon = work / "test.img", output / f"{name}.log", work / "mon.sock"
        shutil.copyfile(base, image)
        bat = work / "AUTOEXEC.BAT"
        bat.write_bytes(("@ECHO OFF\r\n" + "\r\n".join(commands) +
                         "\r\nECHO ETHSMOKE-DONE > COM1\r\n").encode("ascii"))
        subprocess.run(["mcopy", "-o", "-i", str(image), str(bat), "::/AUTOEXEC.BAT"], check=True)
        args = ["qemu-system-i386", "-machine", "pc", "-cpu", "pentium3",
                "-accel", "tcg", "-m", "64", "-display", "none", "-no-reboot",
                "-serial", f"file:{log}", "-monitor", f"unix:{mon},server,nowait",
                "-drive", f"file={image},format=raw,if=floppy", "-boot", "a", "-nic", "none"]
        if nic:
            args += ["-netdev", "user,id=n0", "-device", "i82550,netdev=n0"]
        proc = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        text = ""
        try:
            deadline = time.monotonic() + 75
            while time.monotonic() < deadline and proc.poll() is None:
                if log.exists():
                    text = log.read_text(errors="replace")
                    if "ETHSMOKE-DONE" in text:
                        break
                time.sleep(0.25)
            dump = screen(mon, work / "screen.bin") if proc.poll() is None else "(QEMU exited)"
            (output / f"{name}-screen.txt").write_text(dump)
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
            stderr = proc.stderr.read().decode(errors="replace")
            if stderr:
                print(stderr)
        print(f"--- {name} serial ---\n{text}\n--- {name} screen ---\n{dump}")
        failures = [label for label, passed in checks(text) if not passed]
        if "ETHSMOKE-DONE" not in text:
            failures.append("DOS batch completed")
        if failures:
            raise SystemExit(f"{name}: FAIL: {', '.join(failures)}")
        print(f"{name}: PASS")

def main():
    base, output = map(Path, sys.argv[1:])
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    run(base, output, "ethernet", [
        "JLOAD FRES.JLM",
        "JLOAD I8255X.JLM",
        "ETHINIT > COM1",
        "NTEST > COM1",
        "JLOAD -u I8255X.JLM",
        "NTEST > COM1",
        "JLOAD -u FRES.JLM",
        "JLOAD FRES.JLM",
        "JLOAD I8255X.JLM",
        "ETHINIT > COM1",
        "NTEST > COM1",
        "JLOAD -u I8255X.JLM",
        "JLOAD -u FRES.JLM",
    ], True, lambda s: [
        ("two installs", s.count("ETHINIT: status=0") == 2),
        ("two real packet exchanges", s.count("NTEST: PASS") == 2),
        ("removed signature", "NTEST: found=0" in s),
        ("emulated MAC", s.count("NTEST: mac=52-54-00") == 2),
    ])
    run(base, output, "negative", [
        "JLOAD I8255X.JLM",
        "ETHINIT > COM1",
        "JLOAD FRES.JLM",
        "JLOAD I8255X.JLM",
        "ETHINIT > COM1",
        "JLOAD -u I8255X.JLM",
        "JLOAD -u FRES.JLM",
    ], False, lambda s: [
        ("missing dependency refused", "ETHINIT: status=9" in s),
        ("missing NIC handled", "ETHINIT: status=1" in s),
    ])

    run(base, output, "wireless-gates", [
        "ATHDIAG > COM1",
        "JLOAD FRES.JLM",
        "JLOAD ATH5K.JLM",
        "ATHDIAG > COM1",
        "JLOAD CARDBUS.JLM",
        "JLOAD ATH5K.JLM",
        "ATHDIAG > COM1",
        "ATHINIT > COM1",
        "ATHSCAN > COM1",
        "ATHCONN > COM1",
        "ATHSTOP > COM1",
        "NTEST > COM1",
        "JLOAD -u ATH5K.JLM",
        "JLOAD -u CARDBUS.JLM",
        "JLOAD -u FRES.JLM",
        "ATHDIAG > COM1",
    ], False, lambda s: [
        ("missing dependencies refused and unload", s.count("ATHDIAG: resident=9") == 3),
        ("provider loads", s.count("ATHDIAG: resident=0") == 1),
        ("no bridge", "ATHDIAG: bridges=0" in s),
        ("no powered child", "ATHDIAG: probe=5" in s),
        ("no packet installation", "ATHDIAG: install=5" in s and "ATHINIT: status=5" in s),
        ("scan refused without child", "ATHSCAN: status=5" in s),
        ("connect refused before credentials", "ATHCONN: status=5" in s),
        ("disconnect succeeds", "ATHSTOP: status=0" in s),
        ("no public packet vector", "NTEST: found=0" in s),
        ("clean shutdown", "ATHDIAG: stop=0" in s),
    ])

    run(base, output, "wireless-batch", [
        "CALL WIFIGATE.BAT",
        "TYPE WIFIGATE.TXT > COM1",
        "TYPE CBPRE.TXT > COM1",
        "TYPE CBPOST.TXT > COM1",
        "NTEST > COM1",
    ], False, lambda s: [
        ("batch loaded resident once", s.count("ATHDIAG: resident=0") == 1),
        ("batch unloaded resident", s.count("ATHDIAG: resident=9") == 1),
        ("both PCI snapshots", s.count("CBPROBE: bridges=0") == 2),
        ("scan and install rejected", "ATHSCAN: status=5" in s and "ATHINIT: status=5" in s),
        ("no packet driver", "NTEST: found=0" in s),
        ("shutdown", "ATHSTOP: status=0" in s),
    ])

    run(base, output, "pci-evidence", [
        "PCIEVID > PCIEVID.TXT",
        "TYPE PCIEVID.TXT > COM1",
    ], False, lambda s: [
        ("inventory completed", "PCIEVID: END" in s),
        ("host bridge found", "PCI BDF=00:00.00" in s),
        ("config rows emitted", "00:" in s and "F0:" in s),
    ])

if __name__ == "__main__":
    main()
