# beam

Move your workspace to a sandbox, continue there, and bring the work home.

Beam transfers Git history, staged and unstaged changes, untracked files, and an optional Claude Code session. It rebuilds dependencies. Running databases, servers, and processes stay on the source machine.

## Install

Build with Rust 1.89 or later:

```sh
cargo install --path . --locked
```

## First transfer

1. Open a Git project with at least one commit. Exit its local Claude session before sending it.
2. Run `beam`. Choose a destination when prompted. Beam remembers this choice in your personal configuration.
3. Review the transfer plan. Beam checks tools, file sizes, and supported toolchain pins before upload.
4. Confirm the transfer. Beam opens the remote terminal after startup. Detach with **Ctrl-b**, then **d**.
5. Run `beam down` when you want the work back.

```sh
cd ~/dev/app
beam --to docker --build-image   # Build the bundled image and send the workspace
beam attach                     # Reopen the remote terminal
beam down                       # Return work and remove the sandbox
```

Use `beam --agent shell` to send a workspace without an agent. With no Claude session, the default `auto` mode opens a shell. When several sessions exist, interactive mode offers a selection. `--yes` chooses the most recently modified session; `--session ID` selects one explicitly.

Local files remain editable while the workspace is remote. Avoid running the same agent locally. If local work changes, Beam preserves it and saves remote work in a separate recovery worktree.

## Commands

| Command | Behavior |
|---|---|
| `beam [PATH]` or `beam up [PATH]` | Send the workspace or continue an interrupted upload |
| `beam --dry-run --to TARGET` | Preview and check a transfer without creating it |
| `beam attach [PATH]` | Open the agent or a repair shell |
| `beam down [PATH]` | Return work, save recovery data, and remove the sandbox |
| `beam down --keep` | Return work and keep the sandbox listed and manageable |
| `beam status [--json]` | Show transfer progress, remote readiness, and the next action |
| `beam logs` | Show setup logs and recent terminal output |
| `beam ls [--json]` | List active and retained transfers |
| `beam doctor --to TARGET` | Check the same prerequisites used by upload |
| `beam kill --yes` | Remove the sandbox without returning additional work |
| `beam forget --yes` | Remove the active record after you manually remove a lost sandbox |

For scripts, use an explicit target or personal default with `--yes --detach`. `--allow-large` permits files above the configured limit. `--force` bypasses the local Claude process check.

## What transfers

- **Git:** complete history reachable from the current checkout, staged changes, unstaged changes, untracked files, deletions, executable bits, and symlinks.
- **Extras:** selected ignored files. `.env` and `.env.local` are included when present and ignored. They are **send only** by default. Use `return_extras` to bring their remote edits home.
- **Session:** Claude transcripts, project memory, and file history. User instructions, skills, agents, commands, and filtered settings are copied too.
- **Environment:** only variables listed in `[env] forward`, plus available Claude authentication variables when transferring Claude.

Ignored build output such as `node_modules/`, `target/`, and `.venv/` stays local. Beam detects dependency commands from lockfiles. You can override them with `[sandbox] setup`.

The project keeps its absolute path in the sandbox. Plain SSH uses a private home directory for agent files, so it does not overwrite the host user's Claude configuration.

## Targets

| Target | Requirements |
|---|---|
| `docker` | Local Docker. Use `--build-image` to build the bundled image. |
| `docker+ssh://HOST` | Docker on HOST. `--build-image` builds the image there. |
| `ssh://HOST` | Git, tmux, tar, gzip, and the required project tools. The project path must not exist. Passwordless sudo may be needed to create its parent. |
| `steel` | Steel CLI preview and credentials. Beam creates a computer and installs the base tools. |
| `steel:CHECKPOINT` | The same, using a checkpoint that contains your project toolchain. |

Docker and SSH prerequisites are checked before allocation. Steel checkpoint tools are checked after allocation and before upload. A failed check leaves a saved transfer that you can retry or remove.

The bundled Docker image includes Node.js, pnpm, Python, Git, tmux, and Claude Code. Other toolchains need a custom image or Steel checkpoint. Numeric pins in `.nvmrc`, `.tool-versions`, `mise.toml`, `rust-toolchain.toml`, and `package.json` are checked for supported tools. Dynamic version aliases produce a warning. Beam does not install project toolchains automatically.

### Steel installation

Install the Steel CLI preview and add it to your PATH:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/steel-dev/cli/releases/download/v0.5.0-preview.6/steel-cli-installer.sh | sh
export PATH="$HOME/.steel/bin:$PATH"
```

Run `steel --version` to verify the installation. Add the `export` line to your shell startup file to keep it available.

Set `STEEL_API_KEY` through your usual secret manager or shell environment. Steel computers pause after `[sandbox] timeout`, which defaults to `4h`. `beam attach` and `beam down` resume them.

If allocation is interrupted before Steel returns its computer ID, Beam does not allocate another computer automatically. Inspect `steel computer list --json`, then run `beam --recover-sandbox ID` with the computer from that transfer.

## Claude authentication

Beam cannot copy a macOS Keychain login. Run `claude setup-token` locally and export `CLAUDE_CODE_OAUTH_TOKEN`, or use an existing supported authentication variable. Otherwise, log in after `beam attach`.

“Running” means the remote process is running. It does not confirm authentication or task completion.

## Configuration

Personal defaults live in `~/.config/beam/config.toml`, or under `XDG_CONFIG_HOME`:

```toml
# ~/.config/beam/config.toml
to = "docker+ssh://devbox"
```

Project settings live in `beam.toml`:

```toml
[files]
extras = [".env", "config/dev.json"]   # Send only
return_extras = ["config/local-state"] # Send and return, including deletions
max_file_size = "50MB"

[env]
forward = ["DATABASE_URL"]

[sandbox]
image = "beam-base:latest"
timeout = "4h"                        # Steel only
setup = ["pnpm install --frozen-lockfile"]
```

Extras must be ignored by Git. `return_extras` paths are also uploaded when present. Missing return paths may be created remotely. Returning files use their original hashes to detect local changes. Beam preserves both versions when both sides change.

The size limit covers current Git files, staged blobs, extras, and transferred agent files. Historical Git objects still transfer; the preview states that complete reachable history is included.

An explicit `--to` overrides project `[beam] to`, which overrides your personal default. Setup commands replace detection and must be safe to repeat after failure.

## Recovery

- **Upload interrupted:** run `beam` again. Completed phases are reused.
- **Setup failed:** inspect `beam logs`, fix the environment through `beam attach`, then run `beam` again.
- **Download or cleanup interrupted:** run `beam down` again. A completed local apply is not repeated.
- **Local work changed:** inspect the printed recovery worktree. Your local changes remain in place.
- **Sandbox retained:** use `beam attach` to inspect it, then `beam kill --yes` to remove it. Further sandbox edits do not automatically return.

Beam saves transfer receipts and return packages in `~/.beam/transfers/`. Recovery worktrees live there too. These records can contain transcripts and secrets. Their directories are private. Keep the receipt until you have verified your returned work.

To remove a recovery copy you no longer need, use `git worktree remove --force PATH` with its printed path. Then delete that closed transfer's receipt directory. Do not delete an active transfer record.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
BEAM_E2E_TARGET=docker cargo test --test e2e -- --ignored
cargo test --test e2e_steel -- --ignored
```

Docker tests use a fixture agent. Steel tests verify transport and return using a real computer; they do not verify an authenticated Claude conversation. See [CONTRIBUTING.md](CONTRIBUTING.md) for validation and [SPEC.md](SPEC.md) for the implemented contract.
