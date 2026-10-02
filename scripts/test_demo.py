#!/usr/bin/env python3
"""Check the real demo in a pseudo-terminal and optionally record its output."""
import argparse
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parent.parent


def input_settings(fd):
    """Terminal settings without PENDIN. BSD kernels set it when canonical mode returns with pending input."""
    settings = termios.tcgetattr(fd)
    settings[3] &= ~getattr(termios, "PENDIN", 0)
    return settings


def check(name, *, down=False, size=(80, 24), key=None, interrupt=False, resize=False,
          animation=True, no_color=False, truecolor=True, record=None):
    master, slave = pty.openpty()
    os.set_blocking(master, False)

    def dimensions(width, height):
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))

    dimensions(*size)
    original = input_settings(slave)
    env = dict(os.environ, TERM="xterm-256color", TERM_PROGRAM="unknown")
    for variable in ["NO_COLOR", "BEAM_ANIMATION", "COLORTERM", "TMUX", "STY"]:
        env.pop(variable, None)
    if truecolor:
        env["COLORTERM"] = "truecolor"
    if not animation:
        env["BEAM_ANIMATION"] = "0"
    if no_color:
        env["NO_COLOR"] = "1"
    args = [str(ROOT / "target/debug/beam"), "demo", "--seconds", "6" if record else "1"]
    if down:
        args.append("--down")
    events = []
    data = bytearray()
    started = time.monotonic()
    actions = set()
    decoder = codecs.getincrementaldecoder("utf-8")()
    with tempfile.TemporaryDirectory(prefix="beam-demo-test-") as cwd:
        process = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave, env=env, cwd=cwd)
        try:
            while time.monotonic() - started < (9 if record else 4):
                elapsed = time.monotonic() - started
                if elapsed > 0.2 and "first" not in actions:
                    if key is not None:
                        os.write(master, key)
                    if interrupt:
                        process.send_signal(signal.SIGINT)
                    if resize:
                        dimensions(20, 8)
                    actions.add("first")
                if resize and elapsed > 0.5 and "second" not in actions:
                    dimensions(60, 16)
                    actions.add("second")
                if select.select([master], [], [], 0.03)[0]:
                    try:
                        chunk = os.read(master, 65536)
                        if chunk:
                            data.extend(chunk)
                            decoded = decoder.decode(chunk)
                            if decoded:
                                events.append([round(elapsed, 4), "o", decoded])
                    except BlockingIOError:
                        pass
                if process.poll() is not None and not select.select([master], [], [], 0)[0]:
                    break
            code = process.wait(timeout=1)
            assert code == (130 if interrupt or key == b"\x03" else 0), (name, code, data)
            assert input_settings(slave) == original, f"{name}: terminal input settings changed"
            assert not list(Path(cwd).iterdir()), f"{name}: demo created project files"
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)
    data = bytes(data)
    animated = animation and not no_color and size[0] >= 40 and size[1] >= 12
    if animated:
        assert data.count(b"\x1b[?1049h") == 1, f"{name}: missing alternate screen"
        assert data.count(b"\x1b[?1049l") == 1, f"{name}: alternate screen not restored"
        assert b"\x1b[?25h" in data and b"\x1b[?7h" in data, f"{name}: cursor or wrapping not restored"
        assert b"Esc / q: exit" in data, f"{name}: missing exit hint"
        assert any("\u2800" <= character <= "\u28ff" for character in data.decode()), f"{name}: no braille"
        assert b"No files were transferred" in data, f"{name}: missing summary"
        if key is not None and key != b"\x03":
            assert b"Demo stopped" in data, f"{name}: exit key was not handled"
        if not truecolor:
            assert b"38;2;" not in data and b"38;5;" in data, f"{name}: color mode ignored"
        if resize:
            assert b"Resize to 40 x 12" in data, f"{name}: resize fallback missing"
    else:
        assert b"1049" not in data and b"\x1b[2J" not in data, f"{name}: static preview took over the screen"
        assert b"no files are transferred" in data and b"demo" in data, f"{name}: missing static preview"
        if no_color:
            assert b"\x1b" not in data, f"{name}: NO_COLOR emitted escape sequences"
    if record:
        record.parent.mkdir(parents=True, exist_ok=True)
        header = {"version": 2, "width": size[0], "height": size[1],
                  "title": f"Beam demo: {'return' if down else 'send'}", "env": {"TERM": "xterm-256color"}}
        with record.open("w") as file:
            for event in [header, *events]:
                file.write(json.dumps(event, ensure_ascii=False) + "\n")
    print(f"{name}: passed", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", type=Path, help="Directory for send and return recordings")
    args = parser.parse_args()
    subprocess.run(["cargo", "build", "--locked"], cwd=ROOT, check=True)
    check("send", record=args.record / "send.cast" if args.record else None)
    check("return", down=True, record=args.record / "return.cast" if args.record else None)
    check("escape", key=b"\x1b")
    check("quit", key=b"q")
    check("queued quit", key=b"xq")
    check("control-c", key=b"\x03")
    check("signal interruption", interrupt=True)
    check("resize", resize=True)
    check("small window", size=(20, 8))
    check("motion disabled", animation=False)
    check("color disabled", no_color=True)
    check("indexed color", truecolor=False)


if __name__ == "__main__":
    main()
