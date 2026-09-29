# Contributing to Beam

Use Rust 1.89 or later, Git, and a POSIX shell. Docker is required for local provider tests.

## Check a change

1. Run `cargo fmt --check`.
2. Run `cargo clippy --locked --all-targets -- -D warnings`.
3. Run `cargo test --locked`.
4. Run `BEAM_E2E_TARGET=docker cargo test --test e2e -- --ignored` for transfer changes.
5. Run `cargo test --test e2e_steel -- --ignored` for Steel changes when credentials are available.

The automated workflow runs local tests on Linux and macOS. It runs Docker integration tests on Linux. Cloud tests are explicit because they allocate resources.

Integration fixtures use a temporary HOME. They remove their own sandbox on failure. Git fixtures disable global configuration. Permission assertions compare Git executable bits rather than the machine's umask.

## Code layout

| File | Responsibility |
|---|---|
| `src/plan.rs` | File selection, session selection, preview, and local validation |
| `src/config.rs` | Configuration, setup rules, and toolchain pins |
| `src/state.rs` | Durable records, project pointers, and locking |
| `src/up.rs` | Upload phases and retry |
| `src/down.rs` | Return phases, file merge, and recovery |
| `src/review.rs` | Saved plans, conservative Git combination, review, and undo |
| `src/return_files.rs` | Auxiliary file plans and undo backups |
| `src/agent/` | Agent contract, client adapters, and shared evidence types |
| `src/monitor.rs` | Remote evidence, watching, and phase timings |
| `src/presentation.rs` | Agent-independent status and next actions |
| `src/git.rs` | Git operations and recovery worktrees |
| `src/sandbox.rs` | Provider operations and preflight checks |
| `src/steel.rs` | Steel CLI transport and allocation receipts |
| `src/remote.rs` | Remote script parameters and ownership checks |
| `scripts/` | Shared snapshot, restore, setup, and return scripts |

Keep provider-specific behavior inside the provider modules. Add a saved phase before introducing an operation that cannot safely repeat. Do not delete paths merely because they appear in a transfer plan. Require evidence that this transfer owns them.

When testing interruption, check both sides after retry. Verify local files, returned files, Git index state, resource identity, and cleanup. A successful exit alone does not prove a safe transfer.

## Add an agent

1. Implement `agent::Adapter` in `src/agent/`. Keep client paths and command syntax in that module.
2. Declare capabilities and permitted return paths. Generated defaults use paths relative to the private agent home.
3. Supply discovery, authentication, startup, and observation methods as needed. Register the adapter in `agent::ADAPTERS`.
4. Decode native events into shared observations. A live probe must describe the current process run. Label terminal matches as heuristics.
5. Test session transfer, unknown observations, input resolution, and process restarts. Use `agent::testing::Fixture` as a contract example.
6. Run an authenticated round trip for the actual client. A fixture proves the interface, not support for a client.

Do not add agent-name conditions to shared status, notifications, or return logic. Missing evidence means unknown. A process exit never proves completion.

## Authenticated Claude smoke test

The fixture agent and Steel transport test do not prove that a real conversation resumes.

1. Create a disposable Git project and a real Claude conversation containing a unique test phrase.
2. Exit Claude and configure sandbox authentication through an environment variable.
3. Send the project with `beam --to TARGET`.
4. Verify that Claude recalls the phrase, edits a file, and records the new conversation turns.
5. Run `beam down` and resume Claude locally. Verify the remote turns, file contents, and Git state.
6. Confirm that the sandbox was removed. Record the local and remote Claude versions with the test result.

Use a test credential and a transcript with no private project data. Never commit credentials or raw private transcripts.

## Documentation

`README.md` describes the user workflow. `SPEC.md` describes implemented behavior and marks future work separately. Update both when commands or transfer guarantees change.
