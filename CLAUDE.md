# beam

We are Nikotron 3000 (Niko) and Grave Digger Tamagotchi (Claude). beam moves a Claude Code session up to a sandbox and down again. SPEC.md is the design; keep it true when behavior changes.

## Layout

- `src/` Rust CLI. `up.rs` and `down.rs` are the two flows. Providers are in `sandbox.rs`.
- `scripts/` POSIX sh scripts that run in the sandbox (and `snapshot.sh` also runs locally). Rust fills in their variables in `remote.rs`. Keep them POSIX: the sandbox can be any Linux.
- `tests/e2e.rs` real round trip. It needs Docker: `BEAM_E2E_TARGET=docker+ssh://agent cargo test --test e2e -- --ignored`.
- `tests/e2e_steel.rs` real round trip on a Steel computer: `cargo test --test e2e_steel -- --ignored` (needs STEEL_API_KEY).

## Rules

- The git state moves as commits (`snapshot.sh`). Do not add file-list copies of the worktree.
- `beam down` must never overwrite local work. When both sides changed, keep the remote work aside.
- Tests that run git set `GIT_CONFIG_GLOBAL=/dev/null` so user config (signing, hooks) does not change results.
- The `agent` VM: Docker bridge networking has no internet there. Use `--network host` for builds.
- Steel CLI facts (0.5.0-preview.6): `computer ssh -- CMD` keeps argv but always exits 0 and has no tty; `computer exec` has exit codes but no stdin, and merges stdout and stderr. Interactive `computer ssh` does not exit when the remote shell exits; SIGTERM stops it (SIGINT does not). `src/steel.rs` works around each of these. Test computers must be deleted.
