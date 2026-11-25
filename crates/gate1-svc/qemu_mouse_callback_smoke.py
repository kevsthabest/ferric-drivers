#!/usr/bin/env python3
"""Regression smoke for the INT 33h callback dispatch: inject a
move + both-buttons burst into the resident profile and require
MTEST to finish with callbacks delivered. This is the exact sequence
that killed MTEST with a divide-by-zero on the DynaBook (2026-11-20):
the stub's nested-exec callback left the callback's registers in the
Client_Reg_Struc, the API dispatch then read its function number
from the event mask, and a later dispatch far-called into the
caller's code. If MTEST never finishes, the VGA text screen and CPU
registers are captured so the failure is evidence, not a hang."""
import json
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def qmp_connect(path):
    for _ in range(60):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(str(path))
            return s
        except OSError:
            time.sleep(0.25)
    raise RuntimeError("no qmp")


_qmp_seq = [0]


def qmp_cmd(sock, f, obj):
    # Match responses by id: async event objects carry no id and must
    # not be mistaken for a command's answer (an earlier cut read one
    # line per command and silently desynced, printing {} forever).
    _qmp_seq[0] += 1
    obj = dict(obj)
    obj["id"] = f"probe-{_qmp_seq[0]}"
    sock.sendall((json.dumps(obj) + "\n").encode())
    while True:
        line = f.readline()
        if not line:
            return {}
        try:
            resp = json.loads(line)
        except ValueError:
            continue
        if resp.get("id") == obj["id"]:
            return resp


def send(sock, f, events):
    return qmp_cmd(sock, f, {"execute": "input-send-event",
                             "arguments": {"events": events}})


def btn(name, down):
    return [{"type": "btn", "data": {"button": name, "down": down}}]


def rel(dx, dy):
    return [{"type": "rel", "data": {"axis": "rel_x", "value": dx}},
            {"type": "rel", "data": {"axis": "rel_y", "value": dy}}]


def hmp(sock, f, command):
    resp = qmp_cmd(sock, f, {"execute": "human-monitor-command",
                             "arguments": {"command-line": command}})
    return resp.get("return", str(resp))


def screen_text(sock, f):
    raw = hmp(sock, f, "xp /4000xb 0xb8000")
    data = []
    for line in raw.splitlines():
        if ":" not in line:
            continue
        for tok in line.split(":", 1)[1].split():
            try:
                data.append(int(tok, 16))
            except ValueError:
                pass
    lines = []
    for row in range(25):
        chars = []
        for col in range(80):
            off = (row * 80 + col) * 2
            b = data[off] if off < len(data) else 0x20
            chars.append(chr(b) if 32 <= b < 127 else " ")
        lines.append("".join(chars).rstrip())
    return "\n".join(lines)


def wait_for(log, needle, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if log.exists() and needle in log.read_text(errors="replace"):
            return True
        time.sleep(0.25)
    return False


def main():
    root = Path(__file__).resolve().parents[2]
    base_img = root / "out" / "ferric-dos-pxe.img"
    with tempfile.TemporaryDirectory(prefix="mouse-crash-") as td:
        td = Path(td)
        img = td / "crash.img"
        shutil.copy2(base_img, img)
        for filename, before, after in (
            ("CONFIG.SYS", b"MENUDEFAULT=0,3", b"MENUDEFAULT=1,1"),
            ("AUTOEXEC.BAT", b"\r\nG1LIVE.COM\r\n", b"\r\nMTEST.COM > COM1\r\n"),
        ):
            p = td / filename
            subprocess.run(["mcopy", "-o", "-i", str(img), "::/" + filename, str(p)], check=True)
            data = p.read_bytes()
            assert data.count(before) == 1, filename
            p.write_bytes(data.replace(before, after))
            subprocess.run(["mcopy", "-o", "-i", str(img), str(p), "::/" + filename], check=True)

        log = td / "crash.log"
        sock_path = td / "qmp.sock"
        intlog = td / "int.log"
        proc = subprocess.Popen(
            ["qemu-system-i386", "-cpu", "pentium3", "-machine", "pc",
             "-m", "256", "-accel", "tcg", "-display", "none",
             "-d", "int", "-D", str(intlog),
             "-serial", f"file:{log}",
             "-qmp", f"unix:{sock_path},server,nowait",
             "-drive", f"file={img},format=raw,if=floppy"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            s = qmp_connect(sock_path)
            f = s.makefile("rw")
            f.readline()
            qmp_cmd(s, f, {"execute": "qmp_capabilities"})
            if not wait_for(log, "MTEST: reset ok", 30):
                print("PROBE: MTEST never started")
                return 1
            print("PROBE: MTEST up; burst: move, both buttons held, move, release")
            time.sleep(1.0)
            send(s, f, rel(120, 40))
            time.sleep(1.5)
            send(s, f, btn("left", True))
            time.sleep(1.5)
            send(s, f, btn("right", True))
            time.sleep(2.0)
            send(s, f, rel(-60, -20))
            time.sleep(1.5)
            send(s, f, btn("left", False))
            time.sleep(1.5)
            send(s, f, btn("right", False))
            done = wait_for(log, "MTEST: DONE", 45)
            if intlog.exists():
                hits = [ln for ln in intlog.read_text(errors="replace").splitlines()
                        if "v=00 " in ln]
                print(f"PROBE: divide-error (v=00) deliveries: {len(hits)}")
                for ln in hits[-3:]:
                    print("PROBE:  ", ln)
            print(f"PROBE: MTEST finished = {done}")
            if not done:
                print("PROBE: --- serial tail ---")
                print(log.read_text(errors="replace")[-800:])
                try:
                    print("PROBE: --- registers ---")
                    print(hmp(s, f, "info registers"))
                    print("PROBE: --- guest screen ---")
                    print(screen_text(s, f))
                    # The dump's CS:IP is the faulting instruction. The
                    # segments recur across runs (04C6, 063F, 0752,
                    # 9xBF), so dump code at each of them.
                    for seg in (0x04C6, 0x063F, 0x0752, 0x9FBF):
                        print(f"PROBE: --- mem {seg:04X}:0000 (64B) ---")
                        print(hmp(s, f, f"xp /64xb 0x{seg * 16:x}"))
                        print(f"PROBE: --- mem {seg:04X}:0200 (32B) ---")
                        print(hmp(s, f, f"xp /32xb 0x{seg * 16 + 0x200:x}"))
                except Exception as exc:
                    print("PROBE: screen capture failed:", exc)
                return 2
            print(log.read_text(errors="replace")[-800:])
            return 0
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())
