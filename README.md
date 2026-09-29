# beam

Beam a Claude Code session up from your laptop to a sandbox. Then beam it down again.

```
$ cd ~/dev/app
$ beam --to docker+ssh://mybox     # up: worktree, git state, transcript → sandbox, agent resumes in tmux
$ beam attach                      # open the session
$ beam down                        # stop the agent, bring commits + worktree + transcript home
```

The full design is in [SPEC.md](SPEC.md).

## What moves

- **Git state:** commits (also unpushed ones), staged and unstaged changes, untracked files, deletions, file modes.
- **Extras:** files that git ignores but you need (`.env`, `.env.local` by default; set `[files] extras`).
- **Session:** the Claude Code transcript, its file history, and project memory. User `CLAUDE.md`, skills, agents, and commands are copied when the sandbox does not have them.
- **Env:** only the names in `[env] forward`, plus Claude auth vars (`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`, ...).

Ignored build output (`node_modules/`, `target/`) does not move. The setup commands make it again, and the agent gets a message that tells it what changed.

The sandbox uses the same absolute paths as your machine (for example `/Users/you/dev/app`), so the transcript works without changes.

## Targets

| Target | Needs |
|---|---|
| `docker` | Docker on this machine and the `beam-base` image (`docker build -t beam-base images/base`) |
| `docker+ssh://HOST` | Docker on HOST and the image there |
| `ssh://HOST` | git, tmux, tar, gzip, and `claude` on HOST; passwordless sudo when the mirror path (for example `/Users/you`) must be made |
| `steel` | `STEEL_API_KEY` and the steel preview CLI (0.5.0-preview.6 or later). beam makes a new computer and installs git, tmux, and Claude Code in it (about 10 s) |
| `steel:CHECKPOINT` | The same, but the computer starts from a Steel checkpoint (for example one with your toolchain) |

### Steel notes

- The computer pauses itself after `[sandbox] timeout` (default `4h`). `beam down` and `beam attach` resume it.
- To start faster, make a checkpoint of a ready computer once, then use `to = "steel:<checkpoint-id>"`:
  `steel computer create --wait --use`, run `scripts/steel_bootstrap.sh` in it with `steel computer exec -c`, install your toolchain, then `steel computer checkpoint --name beam-base --wait`.
- `beam attach` opens `steel computer ssh`. The login shell attaches to tmux. Detach with Ctrl-b d.

## Auth

Claude Code on macOS keeps its login in the Keychain, and beam cannot copy it. Run `claude setup-token` and export `CLAUDE_CODE_OAUTH_TOKEN` before `beam`. Without it, log in inside the sandbox after `beam attach`.

## beam.toml

```toml
[beam]
to = "docker+ssh://mybox"

[files]
extras = [".env", "config/dev.json"]
max_file_size = "50MB"

[env]
forward = ["DATABASE_URL"]

[sandbox]
image = "beam-base:latest"
setup = ["pnpm install --frozen-lockfile"]   # default: detected from lockfiles
```

## Tests

```
cargo test                                                    # unit + git round-trip tests
BEAM_E2E_TARGET=docker+ssh://mybox cargo test --test e2e -- --ignored   # real round trip in Docker
cargo test --test e2e_steel -- --ignored                                  # real round trip on Steel
```
