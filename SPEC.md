# beam — Specification

> Move a live coding-agent session from your laptop to a cloud sandbox with one command. Then move it back.

Status: Draft 2 · Owner: Niko · Date: 2026-09-29

**Implementation status:** M1 and M2 are done (Claude Code; `docker`, `docker+ssh://`, and `ssh://` targets; `beam up/down/attach/status/ls/kill/doctor`). Steel computers work too (`steel`, `steel:CHECKPOINT`). Not done yet: M0 manual check with a real Claude login, E2B/Daytona/Modal, Codex, MCP filtering, the `SessionStart` warning hook, and `--fork`.

---

## 1. Pitch

You are in a Claude Code session on your MacBook. The agent is in the middle of a task. You must close the laptop, or the task needs more CPU, or you want it to run all night.

Today, you cannot move that session. The context is on your disk. The dirty worktree is on your disk. The agent transcript is on your disk. When you stop, the work stops.

**beam** moves all of it. Type `beam` in the project directory. Some seconds later, the same session continues in a sandbox, with the same conversation, the same uncommitted changes, and the same task. When you are ready, `beam down` brings it home.

```
        your laptop                                   the cloud
   ┌──────────────────┐                        ┌──────────────────┐
   │  ~/dev/app       │                        │  ~/dev/app       │
   │  ├─ dirty diff   │      $ beam            │  ├─ dirty diff   │
   │  ├─ .env         │  ════════════════▶     │  ├─ .env         │
   │  └─ claude       │     *  *  *  *         │  └─ claude       │
   │     session #42  │                        │     session #42  │
   │     (paused)     │  ◀════════════════     │     (running)    │
   └──────────────────┘     $ beam down        └──────────────────┘
```

Why this is possible now:

1. **A session is small.** It is a git state, a small set of files, some env vars, and one JSONL transcript. It is not a VM image.
2. **Agents heal themselves.** An agent that reads "you are now on Linux, dependencies were reinstalled" will run the build, find a problem, and fix it. beam does not need a perfect copy of the environment. It needs a good copy and an honest message.
3. **Sandboxes are cheap and have APIs.** E2B, Daytona, Modal, Fly, or any SSH host can start in seconds.

---

## 2. Explainer

### 2.1 What is a "session"?

```
   ┌────────────────────────── a coding-agent session ──────────────────────────┐
   │                                                                            │
   │   REPO STATE          WORKTREE DELTA        LOCAL EXTRAS        AGENT      │
   │   ───────────         ──────────────        ────────────        ─────      │
   │   remote URL          staged changes        .env files          transcript │
   │   HEAD sha            unstaged changes      local config        file hist. │
   │   unpushed commits    untracked files       env vars            auth       │
   │   branch              deletions             toolchain versions  settings   │
   │                                                                            │
   │   ── copy ──          ── copy ──            ── allowlist ──     ── copy ── │
   │                                                                            │
   │   REBUILT, NOT COPIED:  node_modules/  target/  .venv/  build caches       │
   └────────────────────────────────────────────────────────────────────────────┘
```

beam copies the small, important parts. beam rebuilds the large, platform-specific parts. A macOS arm64 `node_modules` does not run on Linux, so beam does not copy it.

### 2.2 The same-path trick

Agent transcripts contain absolute paths, for example `/Users/air/dev/app/src/main.rs`. Claude Code also names its session directory from the cwd: `~/.claude/projects/-Users-air-dev-app/`.

We do not rewrite the transcripts. beam makes the sandbox use **the same absolute paths**:

```
   laptop (macOS)                         sandbox (Linux)
   ──────────────                         ───────────────
   HOME=/Users/air                        HOME=/Users/air        ← set by beam
   /Users/air/dev/app                     /Users/air/dev/app     ← same path
   ~/.claude/projects/-Users-air-dev-app  ~/.claude/projects/-Users-air-dev-app
```

It is not beautiful. It works, and it removes a full class of bugs.

### 2.3 The handoff message

When the agent resumes, beam sends one first message:

```
You were moved by beam from macOS (arm64) to Linux (x86_64) in a sandbox.
The worktree and your transcript are the same. These things are different:
- Dependencies were reinstalled with: pnpm install  (exit 0)
- These env vars are not available: AWS_PROFILE
- These MCP servers are disabled: local-postgres (points to localhost)
Verify the environment (build, tests), then continue the task.
```

This message is the "self-heal" contract. The agent does the last 10%.

---

## 3. Goals and non-goals

### Goals

- G1. `beam` in a project directory moves the latest agent session to a sandbox in less than 60 s for a typical repo (without the dependency install time).
- G2. The session resumes with full conversation history and the exact worktree state.
- G3. `beam down` returns the worktree changes and the updated transcript to the laptop.
- G4. Only one copy of a session is "live" at a time.
- G5. Pluggable agents (Claude Code first, Codex second).
- G6. Pluggable providers (SSH first, then Docker, E2B, Daytona, Modal).
- G7. One static Rust binary. No daemon.

### Non-goals

- N1. Live bidirectional sync (use Mutagen for that).
- N2. Memory or process snapshots (CRIU). beam moves state on disk, not running processes.
- N3. Copy of running dev servers, databases, or Docker containers.
- N4. A hosted service. beam uses the provider accounts of the user.
- N5. Windows as a source platform (in v1).

---

## 4. User experience

### 4.1 Commands

```
beam [PATH] [--to <target>] [--session <id>] [--agent claude|codex] [--detach]
    Move the session to a sandbox. PATH defaults to the cwd.

beam down [PATH] [--keep]          (alias: beam back)
    Bring the session home. Stops the sandbox unless --keep is set.

beam attach [PATH]
    Attach the terminal to the remote agent (tmux).

beam status [PATH]
    Show where the session is live: local, or <provider>:<id>.

beam ls
    List all beamed sessions for all projects.

beam kill [PATH]
    Destroy the sandbox. Do not bring changes back. Asks for confirmation.

beam doctor
    Check git, auth tokens, provider credentials, and agent versions.
```

### 4.2 A typical run

```
$ cd ~/dev/app
$ beam --to e2b
▸ agent      claude  session ae4942d0 (updated 2 min ago, 184 turns)
▸ repo       main @ 3f2a1c9  +2 unpushed commits
▸ worktree   7 modified, 2 untracked, 1 deleted
▸ extras     .env, .env.local  (from beam.toml)
▸ skip       node_modules/ (1.2 GB) — will reinstall
! transcript contains 3 reads of .env — it will be uploaded to e2b. Continue? [y/N] y
▸ snapshot   4.1 MB (zstd)
▸ sandbox    e2b:sbx_9k2m  ubuntu-24.04 / node 22 / rust 1.90
▸ upload     ████████████████████ 4.1 MB  1.8 s
▸ setup      pnpm install  ✓ 23 s
▸ resume     claude --resume ae4942d0  (tmux: beam)
✓ Session is live on e2b:sbx_9k2m. Local copy is locked.

  beam attach      open the session
  beam down        bring it home
```

---

## 5. Architecture

### 5.1 Overview

```
                         ┌───────────────────────────────┐
                         │           beam CLI            │
                         │        (clap commands)        │
                         └───────────────┬───────────────┘
                                         │
             ┌───────────────────────────┼───────────────────────────┐
             │                           │                           │
   ┌─────────▼─────────┐      ┌──────────▼──────────┐     ┌──────────▼─────────┐
   │     Snapshot      │      │    Agent adapter    │     │  Provider adapter  │
   │  git bundle       │      │  trait Agent        │     │  trait Provider    │
   │  worktree delta   │      │  ├─ claude          │     │  ├─ ssh            │
   │  extras           │      │  └─ codex           │     │  ├─ docker         │
   │  manifest.json    │      │                     │     │  ├─ e2b            │
   └─────────┬─────────┘      └──────────┬──────────┘     │  ├─ daytona        │
             │                           │                │  └─ modal          │
             │         ┌─────────────────▼───────┐        └──────────┬─────────┘
             └────────▶│       Orchestrator      │◀──────────────────┘
                       │  plan → confirm → pack  │
                       │  → create → upload      │
                       │  → restore → resume     │
                       └────────────┬────────────┘
                                    │
                       ┌────────────▼────────────┐
                       │      Lock + state       │
                       │  .beam/state.json       │
                       │  ~/.beam/sessions.json  │
                       └─────────────────────────┘
```

### 5.2 Beam-up sequence

```
 laptop                              beam                                sandbox
   │                                  │                                     │
   │  $ beam                          │                                     │
   │─────────────────────────────────▶│                                     │
   │                                  │ 1. detect agent + session           │
   │                                  │ 2. check agent is not running       │
   │                                  │ 3. build plan, show summary         │
   │◀──── confirm (secrets warning) ──│                                     │
   │─────────── y ───────────────────▶│                                     │
   │                                  │ 4. pack snapshot (tar.zst)          │
   │                                  │ 5. write lock: live=remote          │
   │                                  │──── provider.create(image) ────────▶│
   │                                  │──── provider.set_secrets(env) ─────▶│
   │                                  │──── provider.upload(snapshot) ─────▶│
   │                                  │──── exec: beam-restore ────────────▶│ 6. unpack at same path
   │                                  │                                     │ 7. git restore
   │                                  │                                     │ 8. toolchain (mise)
   │                                  │                                     │ 9. setup (pnpm i)
   │                                  │◀─────────── setup report ───────────│
   │                                  │──── exec: tmux new claude --resume ▶│ 10. agent resumes
   │                                  │                                     │     + handoff msg
   │◀──────── attach (pty) ───────────│◀═════════════ pty stream ══════════▶│
```

### 5.3 Beam-down sequence

```
 laptop                              beam                                sandbox
   │  $ beam down                     │                                     │
   │─────────────────────────────────▶│──── exec: stop agent (tmux C-c) ───▶│
   │                                  │──── exec: beam-pack --back ────────▶│ bundle new commits
   │                                  │◀─────────── download ───────────────│ + delta + transcript
   │                                  │ verify local HEAD = base HEAD       │
   │                                  │ apply bundle + delta                │
   │                                  │ replace transcript (append-only chk)│
   │                                  │ write lock: live=local              │
   │                                  │──── provider.destroy() ────────────▶│
   │◀──── ✓ home. claude --resume ────│                                     │
```

### 5.4 The remote side

beam does not upload a helper binary. It sends short POSIX `sh` scripts (in `scripts/`), and each provider runs them with `sh -c`. The sandbox needs `git`, `tar`, `gzip`, and `tmux`. The snapshot script is the same on both sides: beam runs it on the laptop for `beam`, and in the sandbox for `beam down`.

Source tree:

```
beam/
├── Cargo.toml
├── SPEC.md
├── images/base/Dockerfile   default sandbox image (git, tmux, node, Claude Code)
├── scripts/
│   ├── snapshot.sh          git state → commits + bundle (runs on both sides)
│   ├── restore.sh           rebuilds the project in the sandbox
│   ├── run.sh               setup + handoff + agent resume (runs in tmux)
│   └── pack_back.sh         packs the sandbox state for beam down
├── src/
│   ├── main.rs              clap entry, status / ls / kill / doctor
│   ├── up.rs                beam up flow
│   ├── down.rs              beam down flow, agent-file merge
│   ├── git.rs               local git: snapshot, checks, apply the work that comes back
│   ├── claude.rs            Claude Code adapter
│   ├── sandbox.rs           providers: docker, docker+ssh, ssh
│   ├── remote.rs            fills in the scripts with their variables
│   ├── pack.rs              tar.gz write and read
│   ├── state.rs             lock + index
│   ├── config.rs            beam.toml + setup detection
│   ├── handoff.rs           the handoff message
│   ├── scan.rs              secret warnings
│   └── util.rs
└── tests/e2e.rs             real round trip against Docker
```

Crates: `clap`, `serde`, `serde_json`, `toml`, `tar`, `flate2`, `sha2`, `anyhow`. There is no async runtime: providers call the `docker` and `ssh` commands (so `~/.ssh/config` applies).

---

## 6. Snapshot

### 6.1 Format

One file: `snapshot.tar.gz`, unpacked in the sandbox at `$HOME/.beam/<session>/`.

```
snapshot.tar.gz
├── manifest.json         information only (source OS, git state, extras, env names, setup)
├── repo.bundle           the git history + the snapshot commits (see 6.3)
├── snapshot.sh           used again by beam down
├── run.sh                the launcher that tmux runs
├── extras/               allowlisted ignored files (.env, ...)
├── home/                 session files, copied into $HOME (always)
│   └── .claude/projects/-Users-air-dev-app/<id>.jsonl, .claude/file-history/<id>/
└── defaults/             user config, copied into $HOME only when it is not there
    └── .claude.json, .claude/settings.json (filtered), .claude/CLAUDE.md, skills/, ...
```

Env values are not in the archive. beam writes them to `$HOME/.beam/<session>/env` (mode 600) through the provider stdin.

### 6.2 Git state as commits

`snapshot.sh` stores the full worktree state as commits, like `git stash` does:

```
            HEAD ◀──────────── W  worktree commit: tree = all files that git does not ignore
              ▲               ╱     (made with a temporary index and `git add -A`)
              └──── I ◀──────╯   I  index commit: tree = `git write-tree` (staged state)
```

`W` has the same shape as a stash entry. The ref is `refs/beam/<session>/up`. The script prints `head`, `branch`, `idx_tree`, `wt_tree`, and `wt_commit`.

### 6.3 Git rules

1. `beam`: bundle the full history of `W` (the sandbox does not need git credentials).
2. Restore: `git init`, fetch the bundle, check out `head` (on `branch`, or detached), then `git read-tree -m -u HEAD <wt_tree>` (worktree) and `git read-tree <idx_tree>` (index). Staged, unstaged, untracked, deleted, file modes, and symlinks all come back.
3. `beam down`: the sandbox makes a new snapshot and bundles only the new objects (`^<sent W>`).
4. Apply: beam makes a local snapshot again. When it is the same as the sent one, beam moves the worktree with a two-way `read-tree -m -u`, moves the branch ref, and sets the index. When it is different, nothing local changes: the remote HEAD goes to branch `beam/<id>` and `W` goes to `git stash list`.

### 6.4 What is not copied

- All paths ignored by `.gitignore`, except the `extras` allowlist.
- Files larger than `max_file_size` (default 50 MB). beam shows them and stops, unless `--allow-large`.
- Symlinks that point outside the repo. beam shows a warning.

---

## 7. Agent adapters

```rust
trait Agent {
    fn kind(&self) -> AgentKind;
    fn detect(&self, project: &Path) -> Result<Option<SessionRef>>;   // latest session for cwd
    fn is_running(&self, s: &SessionRef) -> Result<bool>;             // pgrep / lsof on the jsonl
    fn session_files(&self, s: &SessionRef) -> Result<Vec<PathBuf>>;
    fn config_files(&self) -> Result<Vec<ConfigFile>>;                // settings, CLAUDE.md, skills
    fn auth(&self) -> Result<AuthPlan>;                               // env vars / files to inject
    fn install_cmd(&self, version: &str) -> String;
    fn resume_cmd(&self, s: &SessionRef, handoff: &str) -> Vec<String>;
    fn merge_back(&self, local: &Path, remote: &Path) -> Result<()>;
}
```

### 7.1 Claude Code

| Item | Location |
|---|---|
| Transcript | `~/.claude/projects/<cwd with / → ->/<uuid>.jsonl` |
| File history | `~/.claude/file-history/<uuid>/` |
| User settings | `~/.claude/settings.json`, `~/.claude/CLAUDE.md`, `~/.claude/skills/` |
| Project settings | In the repo (`.claude/`, `CLAUDE.md`), copied with the worktree |
| MCP config | `~/.claude.json` and `.mcp.json` — filtered (see 7.3) |
| Auth | macOS Keychain. **Cannot be copied.** Use `claude setup-token` → `CLAUDE_CODE_OAUTH_TOKEN`, or `ANTHROPIC_API_KEY` |
| Resume | `claude --resume <uuid> "<handoff>"` |
| Detect | Newest `*.jsonl` by mtime in the project directory |

### 7.2 Codex

| Item | Location |
|---|---|
| Transcript | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (find by `cwd` field in the first line) |
| Config | `~/.codex/config.toml`, `AGENTS.md` in the repo |
| Auth | `~/.codex/auth.json` or `OPENAI_API_KEY` |
| Resume | `codex resume <id>` |

### 7.3 MCP servers and hooks

beam reads the MCP config and classifies each server:

- `stdio` with a command that exists in the image → keep.
- `http`/`sse` to a public URL → keep.
- Anything to `localhost`, `127.0.0.1`, or a macOS-only binary → disable, and list it in the handoff message.

Hooks that call macOS-only tools (`osascript`, `pbcopy`, `terminal-notifier`) are disabled the same way.

---

## 8. Provider adapters

```rust
impl Target  { fn create(&self, name, image, session) -> Result<Sandbox> }
impl Sandbox {
    fn exec(&self, script: &str) -> Result<String>;               // sh -c <script>
    fn exec_input(&self, script: &str, data: &[u8]) -> Result<String>;
    fn exec_file(&self, script: &str, file: &Path) -> Result<String>;   // upload = tar x from stdin
    fn exec_bytes(&self, script: &str) -> Result<Vec<u8>>;        // download = cat
    fn interactive(&self, script: &str) -> Result<()>;            // tmux attach
    fn destroy(&self, cleanup: &str) -> Result<()>;
}
```

| Provider | Create | Upload | Attach | Order |
|---|---|---|---|---|
| `ssh://host` | not needed (existing host; sudo only to make the mirror path) | ssh stdin | `ssh -t host tmux attach` | M1 ✓ |
| `docker` | `docker run -d` | `docker exec -i` stdin | `docker exec -it` | M2 ✓ |
| `docker+ssh://host` | `ssh host docker run -d` | same, through ssh | `ssh -t host docker exec -it` | M2 ✓ |
| `steel` / `steel:CKPT` | `steel computer create` / `steel checkpoint restore` (`--wait --auto-pause`), then `steel_bootstrap.sh` | `steel computer ssh` stdin; exit code through an rc file read with `exec` | `.bashrc` hook + interactive `steel computer ssh` | ✓ |
| `e2b` | REST API | files API | pty websocket | M3 |
| `daytona` | REST API | files API | ssh gateway | M4 |
| `modal` | Sandbox API | volume / exec | exec pty | later |

### 8.1 Sandbox image

`ghcr.io/<org>/beam-base`: Ubuntu 24.04, git, tmux, mise, build-essential, node LTS, python 3, rust stable, `claude`, `codex`. A user can set `image = "..."` in `beam.toml`.

At restore, `mise install` reads `.tool-versions`, `mise.toml`, `.nvmrc`, `rust-toolchain.toml` and installs the exact versions.

### 8.2 Agent in tmux

The agent always runs in a tmux session with the name `beam`. So:

- The agent continues when the laptop disconnects.
- `beam attach` works again and again.
- `beam down` can stop the agent in a clean way (`tmux send-keys C-c`, then wait for the process to stop).

---

## 9. Setup detection

| File found | Command |
|---|---|
| `pnpm-lock.yaml` | `pnpm install --frozen-lockfile` |
| `package-lock.json` | `npm ci` |
| `yarn.lock` | `yarn install --immutable` |
| `bun.lock` / `bun.lockb` | `bun install --frozen-lockfile` |
| `uv.lock` | `uv sync` |
| `poetry.lock` | `poetry install` |
| `requirements.txt` | `python -m venv .venv && .venv/bin/pip install -r requirements.txt` |
| `Cargo.lock` | `cargo fetch` |
| `go.sum` | `go mod download` |
| `Gemfile.lock` | `bundle install` |

`setup` in `beam.toml` replaces the detection. A failed setup does **not** stop the beam. beam writes the error into the handoff message, and the agent fixes it.

---

## 10. Lock and state

Only one copy of a session is live. beam writes the state in two places:

- `<project>/.beam/state.json` (beam adds `.beam/` to `.git/info/exclude`)
- `~/.beam/sessions.json` (index for `beam ls`)

```
             beam                         beam down
   LOCAL ─────────────▶ BEAMING ──▶ REMOTE ─────────▶ RETURNING ──▶ LOCAL
     ▲                    │                              │
     └──── failure ───────┘                              └── failure: stays REMOTE
```

While the state is `REMOTE`:

- beam installs a Claude Code `SessionStart` hook (project-level) that prints a warning: "This session is live on e2b:sbx_9k2m. Use `beam down` first."
- `beam` again from the same project refuses to run.

### 10.1 Conflicts on beam down

| Case | Action |
|---|---|
| Local worktree not changed since beam | Apply the remote changes. |
| Local worktree changed | Stop. Save the remote changes as branch `beam/<session>` + a stash. Tell the user. |
| Local transcript changed (someone resumed locally) | Stop. Save the remote transcript as a new session file. Tell the user. |
| Remote transcript is not an extension of the local one | Same as above. |

beam never overwrites local work without a confirmation.

---

## 11. Configuration

`beam.toml` in the project root (optional):

```toml
[beam]
to = "e2b"                          # default target
agent = "claude"

[files]
extras = [".env", ".env.local", "config/dev.json"]
exclude = ["fixtures/huge/**"]
max_file_size = "50MB"

[env]
forward = ["DATABASE_URL", "STRIPE_KEY"]   # names only; values from local env

[sandbox]
image = "ghcr.io/niko/beam-base:latest"
cpu = 4
memory = "8GB"
timeout = "12h"

setup = ["pnpm install --frozen-lockfile", "pnpm db:migrate"]
```

`~/.config/beam/config.toml` holds provider credentials references (never raw keys: use env var names or `op://` / keychain references) and user defaults.

---

## 12. Security and privacy

1. **Transcripts contain secrets.** A transcript contains all that the agent read. beam scans the transcript and the extras for known secret patterns (AWS keys, `sk-`, `ghp_`, private keys, `.env` reads) and shows a warning before the upload. `--yes` skips the question, but beam still prints the warning.
2. **Env values never go in the snapshot.** They go through the provider secrets API, or through `exec` stdin for SSH.
3. **Auth tokens** are long-lived. beam recommends `claude setup-token` and shows where the token is stored in the sandbox.
4. **Git credentials.** beam prefers a short-lived GitHub token (`gh auth token`) forwarded as env, over copying SSH keys. beam never copies `~/.ssh`.
5. **Destroy by default.** `beam down` destroys the sandbox. `--keep` is explicit.
6. **Snapshots on disk** are deleted after a successful upload.

---

## 13. Edge cases

| Case | Behavior |
|---|---|
| Agent still running locally | Stop with message: "exit claude first" (`--wait` waits for exit). |
| No git repo | Support it: tar the full directory with `.gitignore`-style rules from `beam.toml`. |
| Detached HEAD | Supported. `branch` is null. |
| Submodules | Clone recursive; bundle each submodule with unpushed commits. |
| Git LFS | `git lfs pull` in restore; warn when LFS is not installed. |
| Very large untracked files | Stop, list them, suggest `exclude`. |
| Two sessions for one project | Use the newest; `--session` to select. `beam` prints the one it chose. |
| Network drops during upload | Retry with backoff; state goes back to `LOCAL`. |
| Sandbox timeout while REMOTE | `beam status` shows `LOST`; `beam down` fails with a clear message; transcript is still local (old version). |

---

## 14. Testing

- **Unit:** manifest serialization, path encoding (`/Users/air/dev/app` → `-Users-air-dev-app`), setup detection, MCP classification, secret scanner.
- **Git round-trip:** fixture repos with each case (unpushed commits, staged + unstaged in the same file, deletions, renames, untracked, no upstream, submodule). Pack → unpack in a temp dir → compare `git status --porcelain` and file hashes.
- **Agent fixtures:** real (redacted) Claude and Codex transcripts; test `detect`, `session_files`, `merge_back`.
- **End-to-end:** Docker provider in CI. Beam a fixture repo with a stub agent command, verify worktree, run `beam down`, verify local state is the same.
- **Provider contract tests:** one test suite that every provider must pass. Real providers run in a nightly job with real credentials.

---

## 15. Milestones

| # | Scope | Done when |
|---|---|---|
| M0 | Verify assumptions: Claude `--resume` with a copied transcript on another machine at the same path; Codex `resume`; `setup-token` in a headless box | Manual test passes on a Linux VM |
| M1 | Snapshot + restore + Claude adapter + SSH provider + tmux attach | `beam --to ssh://box` resumes a real session |
| M2 | `beam down`, lock, conflicts, Docker provider, E2E tests in CI | Round trip is lossless on fixture repos |
| M3 | E2B provider, secrets API, secret scanner, handoff message with setup report | `beam` with no flags works for a pnpm repo |
| M4 | Codex adapter, Daytona provider, MCP/hook filtering | Codex round trip works |
| M5 | `beam ls`, `doctor`, release (Homebrew tap, static Linux binaries) | Public 0.1.0 |

Size estimate: M1–M3 is ~2k lines of Rust.

---

## 16. Open questions

1. Does Claude Code accept a transcript created by a different version? Do we pin the agent version in the sandbox to the local version?
2. Is the same-path trick always possible? Some providers do not allow `/Users/...` or a custom `HOME`. The fallback is a path rewrite in the JSONL (a `cwd` field and the tool inputs/outputs) — how risky is it?
3. Do we want `beam --to <provider> --fork`? It makes a copy and does not lock, so two agents can try two approaches.
4. Do we sync back during the remote run (periodic `git push` to a `beam/*` branch) as a safety net if the sandbox is lost?
5. Can the user watch the session from a phone? (The provider web terminal, or a small read-only web view of the transcript.)
