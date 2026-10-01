#!/usr/bin/env python3
"""Exercise the real progress renderer in a PTY; no sandbox or credentials needed."""
import argparse
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import termios
import time

ROOT = Path(__file__).resolve().parent.parent


def check(name, effect="auto", terminal="ghostty", cancel=False, resize=False, down=False, message=False):
    master, slave = pty.openpty()
    os.set_blocking(master, False)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
    settings = termios.tcgetattr(slave)
    settings[3] &= ~termios.ECHO
    termios.tcsetattr(slave, termios.TCSANOW, settings)
    env = dict(os.environ, TERM="xterm-256color", TERM_PROGRAM=terminal,
               COLORTERM="truecolor", BEAM_EFFECT=effect)
    for key in ["NO_COLOR", "TMUX", "STY", "KITTY_WINDOW_ID", "BEAM_ANIMATION"]:
        env.pop(key, None)
    binary = ROOT / "target/debug/examples/ambient"
    process = subprocess.Popen([str(binary)] + (["down"] if down else []) + (["message"] if message else []),
                               stdin=slave, stdout=slave, stderr=slave, env=env)
    data = bytearray()
    started = time.monotonic()
    acted = False
    try:
        # This type-ahead must remain available, not be eaten by capability queries.
        os.write(master, b"preserved input\n")
        while time.monotonic() - started < 10:
            if not acted and time.monotonic() - started > 1.5:
                if cancel:
                    process.send_signal(signal.SIGINT)
                if resize:
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 20, 0, 0))
                acted = True
            if select.select([master], [], [], 0.05)[0]:
                try:
                    chunk = os.read(master, 65536)
                    if chunk:
                        data.extend(chunk)
                except BlockingIOError:
                    pass
            if process.poll() is not None and not select.select([master], [], [], 0)[0]:
                break
        assert process.wait(timeout=1) == (130 if cancel else 0), name
        os.set_blocking(slave, False)
        assert os.read(slave, 4096) == b"preserved input\n", f"{name}: input was consumed"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)
    data = bytes(data)
    assert b"1049" not in data and b"\x1b[2J" not in data, f"{name}: screen takeover"
    assert b"Beam display preview" in data and (b"preview sandbox" in data or down)
    graphics = effect != "off" and terminal == "ghostty" and effect != "shader"
    if graphics:
        ids = re.findall(rb"a=T[^;]*,i=(\d+)", data)
        assert ids and len(set(ids)) == 1, f"{name}: placement id changed"
        assert b"C=1,z=-1,q=2" in data, f"{name}: foreground image/cursor movement"
        cleanup = b"a=d,d=I,i=" + ids[0] + b",q=2"
        assert cleanup in data, f"{name}: image not removed"
        assert b"a=T" not in data.split(cleanup, 1)[1], f"{name}: image reappeared after cleanup"
    elif effect == "shader":
        marker = b"#a980ee" if down else b"#0bcae1"
        assert marker in data and b"\x1b]112\x1b\\" in data, f"{name}: shader cleanup"
    else:
        assert b"\x1b_G" not in data and b"\x1b]12;" not in data, f"{name}: unexpected effect"
    if message:
        assert b"mid-transfer message remains visible" in data, name
    if not cancel:
        assert b"Preview complete" in data, f"{name}: summary disappeared"
    print(f"{name}: passed", flush=True)
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path, help="Save one graphics capture for inspection")
    args = parser.parse_args()
    subprocess.run(["cargo", "build", "--locked", "--example", "ambient"], cwd=ROOT, check=True)
    data = check("graphics")
    if args.capture:
        args.capture.write_bytes(data)
    check("off", effect="off")
    check("unknown terminal", terminal="unknown")
    check("cancellation", cancel=True)
    check("resize", resize=True)
    check("shader", effect="shader")
    check("shader return", effect="shader", down=True)
    check("intervening output", message=True)


if __name__ == "__main__":
    main()
