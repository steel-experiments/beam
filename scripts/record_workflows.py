#!/usr/bin/env python3
"""Record real Beam commands against local snapshot fixtures, without a sandbox or credentials."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


class Fixture:
    def __init__(self, directory):
        self.fixture_home = directory / "home"
        self.project = self.fixture_home / "dev/app"
        self.project.mkdir(parents=True)
        self.environment = dict(os.environ, HOME=str(self.fixture_home), TERM="dumb", NO_COLOR="1",
                                GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
        for key in ["XDG_CONFIG_HOME", "DAYTONA_API_KEY", "STEEL_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN",
                    "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]:
            self.environment.pop(key, None)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Beam demo")
        self.git("config", "user.email", "demo@example.invalid")
        (self.project / ".gitignore").write_text(".beam/\n.env\n")
        (self.project / "README.md").write_text("Hello from the local workspace.\n")
        (self.project / "beam.toml").write_text("[sandbox]\nsetup = []\nverify = ['git diff --check']\n")
        self.git("add", ".")
        self.git("commit", "-qm", "Create the example workspace")

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.project,
                                       env=self.environment, stderr=subprocess.STDOUT).decode().strip()

    def snapshot(self, name, bundle="-", exclude=""):
        result = subprocess.check_output(["sh", str(ROOT / "scripts/snapshot.sh"), str(self.project),
                                          f"refs/beam/{name}", str(bundle), exclude], env=self.environment)
        return dict(line.split("=", 1) for line in result.decode().strip().splitlines())

    def receive(self, transfer, conflict=False):
        receipt = self.fixture_home / ".beam/transfers" / transfer
        receipt.mkdir(parents=True)
        sent = self.snapshot(transfer + "/sent")
        (self.project / "README.md").write_text("Hello from the local workspace.\nChange from the sandbox.\n")
        (self.project / "remote.txt").write_text("A new file to review.\n")
        self.git("add", "remote.txt")
        remote = self.snapshot(transfer + "/back", receipt / "repo.bundle", sent["wt_commit"])
        (receipt / "info").write_text("".join(f"{key}={value}\n" for key, value in remote.items()))
        with tarfile.open(receipt / "back.tar.gz", "w:gz") as archive:
            for filename in ["info", "repo.bundle"]:
                archive.add(receipt / filename, arcname=filename)
        self.git("reset", "--hard", sent["head"])
        if conflict:
            (self.project / "README.md").write_text("Hello from the local workspace.\nA separate local edit.\n")
        state = {"version": 2, "transfer_id": transfer, "session_id": "", "agent": "shell",
                 "project_root": str(self.project), "agent_cwd": str(self.project),
                 "home": str(self.fixture_home), "target": "ssh://fixture", "sandbox": None,
                 "stage": "", "tmux": "fixture", "sent": sent, "sent_files": {},
                 "phase": "downloaded", "created_at": int(time.time()) - 90}
        (receipt / "state.json").write_text(json.dumps(state))
        pointer = self.project / ".beam/state.json"
        pointer.parent.mkdir(exist_ok=True)
        pointer.write_text(json.dumps({"version": 2, "record": str(receipt / "state.json")}))
        return receipt

    @staticmethod
    def close(receipt):
        # Fixture-only transition: no provider exists to clean up. The recorded commands never contact one.
        path = receipt / "state.json"
        state = json.loads(path.read_text())
        state["phase"] = "closed"
        path.write_text(json.dumps(state))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "docs/demos")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cargo", "build", "--locked"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix="beam-workflows-") as directory:
        fixture = Fixture(Path(directory))
        events = []
        timestamp = 0.0

        def emit(text):
            nonlocal timestamp
            # Keep private temporary paths out of the shared recording. Preserve the rest of the actual output.
            text = text.replace(directory, "/tmp/beam-example")
            events.append([timestamp, "o", text.replace("\n", "\r\n")])
            timestamp += 2.5

        def beam(*command, expected=0):
            emit("$ beam " + shlex.join(command) + "\n")
            result = subprocess.run([str(ROOT / "target/debug/beam"), *command], cwd=fixture.project,
                                    env=fixture.environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            assert result.returncode == expected, result.stdout.decode()
            emit(result.stdout.decode())

        emit("Local fixtures: plan, saved return, review, undo, and conflict recovery.\n"
             "No sandbox is allocated. Provider transfer and cleanup are not demonstrated.\n\n")
        # This intentionally stops at the missing credential check, after rendering the compact plan.
        beam("up", "--to", "daytona", "--agent", "shell", "--yes", "--detach", expected=1)
        emit("\nThe following fixture starts with an already downloaded return package.\n")
        receipt = fixture.receive("clean-return")
        beam("down", "--review", "--detach")
        beam("review", "--diff")
        beam("review", "--apply", "--keep")
        assert (fixture.project / "remote.txt").read_text() == "A new file to review.\n"
        beam("undo")
        assert not (fixture.project / "remote.txt").exists()
        assert (fixture.project / "README.md").read_text() == "Hello from the local workspace.\n"
        fixture.close(receipt)

        emit("\nA second downloaded fixture changes the same path locally and remotely.\n")
        receipt = fixture.receive("conflict-return", conflict=True)
        beam("down", "--keep", "--detach", expected=1)
        assert "A separate local edit." in (fixture.project / "README.md").read_text()
        fixture.close(receipt)
        beam("status")
        beam("review")
        beam("review", "--diff")
        emit("$ # Integrate the saved work through your editor, then acknowledge recovery.\n")
        (fixture.project / "README.md").write_text(
            "Hello from the local workspace.\nA separate local edit.\nChange from the sandbox.\n")
        (fixture.project / "remote.txt").write_text("A new file to review.\n")
        beam("review", "--resolved")
        beam("status")

        header = {"version": 2, "width": 120, "height": 32,
                  "title": "Beam workflows with local snapshot fixtures", "env": {"TERM": "xterm-256color"}}
        with (args.output / "workflows.cast").open("w") as file:
            for event in [header, *events]:
                file.write(json.dumps(event, ensure_ascii=False) + "\n")
    print("Recorded workflows.cast; local apply, undo, and conflict preservation checks passed.")


if __name__ == "__main__":
    main()
