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


def check(name, effect="auto", terminal="ghostty", cancel=False, resize=False, down=False, message=False,
          animation=True, no_color=False, multiplexed=False, show=False):
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
    if not animation:
        env["BEAM_ANIMATION"] = "0"
    if no_color:
        env["NO_COLOR"] = "1"
    if multiplexed:
        env["TMUX"] = "/fixture/tmux"
    binary = ROOT / "target/debug/examples/ambient"
    arguments = (["down"] if down else []) + (["message"] if message else []) + (["show"] if show else [])
    process = subprocess.Popen([str(binary)] + arguments,
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
    graphics = effect != "off" and terminal == "ghostty" and effect != "shader" and animation and not no_color and not multiplexed
    if graphics:
        ids = re.findall(rb"a=T[^;]*,i=(\d+)", data)
        assert ids and len(set(ids)) == 1, f"{name}: placement id changed"
        assert b"C=1,z=-1,q=2" in data, f"{name}: foreground image/cursor movement"
        cleanup = b"a=d,d=I,i=" + ids[0] + b",q=2"
        assert cleanup in data, f"{name}: image not removed"
        assert b"a=T" not in data.split(cleanup, 1)[1], f"{name}: image reappeared after cleanup"
    elif effect == "shader" and animation and not no_color and not multiplexed:
        marker = b"#a980ee" if down else b"#0bcae1"
        assert marker in data and b"\x1b]112\x1b\\" in data, f"{name}: shader cleanup"
    else:
        assert b"\x1b_G" not in data and b"\x1b]12;" not in data, f"{name}: unexpected effect"
    if message:
        assert b"mid-transfer message remains visible" in data, name
    # UTF-8 for the braille block, U+2800 to U+28FF.
    braille = re.compile(rb"\xe2[\xa0-\xa3][\x80-\xbf]")
    animated = show and effect != "off" and animation and not no_color
    if animated:
        erase = b"\r\x1b[10A\x1b[J"
        assert b"Your machine" in data and braille.search(data), f"{name}: no transfer animation"
        assert erase in data, f"{name}: animation not erased"
        assert not braille.search(data.rsplit(erase, 1)[1]), f"{name}: animation drawn after erase"
        if message:
            # The message prints after the strip is lifted, and the strip returns below it.
            after = data.split(b"mid-transfer message", 1)[1]
            assert data.split(b"mid-transfer message", 1)[0].endswith(erase + b"A "), f"{name}: message not above strip"
            assert braille.search(after.split(b"Preview complete", 1)[0]), f"{name}: strip did not return"
    else:
        assert b"Your machine" not in data, f"{name}: unexpected transfer animation"
    if not cancel:
        assert b"Preview complete" in data, f"{name}: summary disappeared"
    if not animation or no_color:
        assert b"\r\x1b[2K" not in data, f"{name}: progress still redraws"
        assert re.search(rb"previewing transfer effect (?:\x1b\[[0-9;]*m)*\d+\.\ds", data), f"{name}: missing final elapsed time"
        assert b"\x1b]9;4;3;" not in data, f"{name}: animated tab indicator"
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
    check("all motion disabled", animation=False)
    check("shader motion disabled", effect="shader", animation=False)
    check("plain color output", no_color=True)
    check("multiplexer", multiplexed=True)
    check("transfer animation", terminal="unknown", show=True)
    check("transfer animation return", terminal="unknown", show=True, down=True)
    check("transfer animation with output", terminal="unknown", show=True, message=True)
    check("transfer animation with graphics", show=True)
    check("transfer animation cancellation", terminal="unknown", show=True, cancel=True)
    check("transfer animation resize", terminal="unknown", show=True, resize=True)
    check("transfer animation effects off", effect="off", show=True)
    check("transfer animation motion disabled", show=True, animation=False)


if __name__ == "__main__":
    main()
