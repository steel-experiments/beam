# Beam implementation contract

This document describes implemented behavior. Future ideas live in the final section.

## Product scope

Beam moves a Git workspace between a local machine and a sandbox. A Claude Code session is optional. The default agent mode resumes an existing session or opens a shell when none exists.

The transfer includes the current checkout's reachable history, index, worktree, selected ignored files, and agent files. It recreates dependencies through setup commands. It does not copy process memory, running services, databases, or installed toolchains.

Supported source platforms are Linux and macOS. The repository needs at least one commit. Submodules are rejected before transfer. Detected Git LFS configuration is rejected because its separate object store does not transfer. Windows is not supported.

## User workflow

`beam` builds a plan, checks prerequisites, asks for confirmation, and sends the workspace. The send confirmation uses `[Y/n]`: Enter accepts, and `n` cancels. Interactive mode attaches after startup. `--detach` returns to the local shell.

When no target is configured, interactive mode asks for one and saves it as a personal default. Noninteractive commands must supply a target or configured default. Target precedence is `--to`, project `[beam] to`, then personal `to`.

Claude sessions are discovered in the invocation directory, with a project-root fallback for automatic selection. Interactive mode offers a choice when several sessions exist. Noninteractive mode uses the latest modified transcript. Explicit session selection fails if the session is absent.

The plan leads with the project, destination, and selected session. It groups transfer and return policies before supporting details: changed paths, complete history, extras, environment names, setup commands, session ID, and compatibility warnings. Values of environment variables are not printed.

`beam --dry-run` and `beam doctor` use the same plan and provider checks. They do not create a transfer sandbox. Docker checks use a temporary container that is removed after checking.

## State and ownership

The authoritative record is `~/.beam/transfers/<transfer>/state.json`. `<project>/.beam/state.json` points to it. `beam ls` derives its list from authoritative records. If the project pointer is missing, Beam finds the active record by project path. Mutating commands restore the pointer.

Record writes use a temporary file, file synchronization, atomic rename, and directory synchronization. Each project has an operating-system file lock. The lock releases when the process exits, including a crash. It serializes Beam commands; it does not lock editors or local agents.

Records use schema version 2. Unsupported versions fail explicitly. Version 1 records migrate on their next save. Old SSH transfers have no verifiable ownership markers, so automatic deletion is disabled for them.

The upload phases are:

```text
planned → allocating → created → prepared → uploaded → restored → starting → remote
```

The return phases are:

```text
remote → returning → downloaded → applied → closed
                                      └→ retained → closed
```

Errors retain the last completed phase and an error message. Repeating the corresponding command continues the transfer.

Status and upload results share the same interpretation of saved phases and live remote state. Each summary recommends one next command. Return and retention phases take precedence over remote process readiness. Status JSON retains the operation phase and adds `summary`, `next_action`, and `saved_recovery`.

Status also finds the latest closed receipt with conflicts for the current project. It shows saved recovery even when another transfer is active. This lookup does not modify receipts. Beam cannot detect manual conflict resolution. `beam review --resolved` writes a separate acknowledgment. Status omits acknowledged recovery, including retained transfers. Saved copies remain intact.

Docker allocation uses a deterministic container name and transfer label. An existing container must have the expected label. Steel allocation writes its response to a durable receipt. If the response is ambiguous, the user identifies the computer with `--recover-sandbox ID`; Beam does not allocate a duplicate automatically.

SSH preparation claims new directories with ownership markers. Existing unowned paths are rejected. Cleanup checks markers before removing paths. Agent files use a private home under the transfer stage on SSH. Shared user files are not overwritten or removed.

## Git snapshots

`scripts/snapshot.sh` runs both locally and remotely. It creates an index commit and a worktree commit using a temporary Git index. The worktree commit has the original HEAD and index commit as parents.

Upload bundles all objects reachable from the worktree commit. Return bundles exclude objects reachable from the sent snapshot. The branch, HEAD, index tree, and worktree tree are recorded separately.

Restore reconstructs the checkout, worktree, and index. Staged and unstaged changes remain distinct. Git preserves executable bits, not all filesystem permission bits. Symlinks in Git preserve their link targets; external targets do not transfer.

Before applying returned Git changes, Beam saves a separate recovery worktree. It compares the current local snapshot with the sent snapshot. Separate path changes can merge when both sides retain the sent commit and branch. Different changes to the same path or diverging history keep remote work in the recovery worktree. Local ignored files that would be overwritten also prevent apply. If local state already matches the returned state, a retry skips the apply.

Recovery worktrees preserve returned HEAD, index, worktree, and untracked files. They use a detached checkout and do not overwrite user branches or stashes.

## Archive and file policies

The outgoing archive contains the Git bundle, remote scripts, selected extras, agent files, filtered defaults, and a manifest. Environment values travel separately through standard input into a private remote file.

The same transfer plan determines the preview, size checks, and file inputs. The configured size limit covers current Git files, staged blobs, extras, and agent files. Historical objects are not individually limited. The archive size is shown after packing.

Default extras are existing ignored `.env` and `.env.local` files. Other extras must be specified explicitly and ignored by Git. Paths must be relative and cannot include parent traversal. Extras cannot be symlinks. User configuration links within HOME are copied as regular content, with cycle detection.

`extras` are send-only. `return_extras` are sent and returned. Return paths can include directories and can be absent before upload.

Returning extras use a three-way comparison:

| Local change | Remote change | Result |
|---|---|---|
| No | Yes | Apply remote file or deletion |
| Yes | No | Preserve local file or deletion |
| Same content | Same content | Keep the existing file |
| Yes | Different change | Preserve local work and save the remote file under the receipt |

Agent files use the same content comparison. Agent-file deletions are not propagated. The adapter declares their return scope. For Claude, it includes the project session directory and file history.

A return package is immutable after successful remote creation. Downloads stream to a private temporary file and rename after success. Extraction streams regular files to disk. Links, traversal, duplicate paths, and unexpected archive entries are rejected.

The `applied` phase is saved before sandbox cleanup. Cleanup retries do not reapply files. A conflict return reports a nonzero exit status after preserving recovery data.

## Setup and readiness

Lockfile detection selects one command per ecosystem. Each rule contains its lockfile, ecosystem, command, and required tool. Explicit setup commands replace detection.

Supported numeric toolchain pins are checked against the destination. Dynamic aliases and unsupported tools produce warnings. Transfer tools and the agent executable remain hard prerequisites. For adapters with `environment_repair`, project tool and version checks run after upload. Missing project tools are repair context for the agent. Shell transfers keep the full preflight requirement. Custom images and Steel checkpoints can supply tools in advance.

Remote project prerequisites, setup, and verification run in order and stop on the first failure. Output is saved in a persistent setup log. A repair-capable agent starts even when these checks fail. Its handoff contains the saved prerequisite checks, setup and verification commands, original task context, and the last 80 lines from each attempted check log. Logs are diagnostic data, not instructions.

The launcher exports `BEAM_CHECK`, a generated script containing the saved checks. The agent repairs the environment without weakening checks or project version requirements, then invokes `sh "$BEAM_CHECK"`. The script reruns prerequisites, setup, and verification in the session directory. It clears repair status only on success. Completion reports cannot mark the environment ready. An empty verification list remains explicitly unverified. Check invocations use a separate lock to prevent concurrent setup.

Adapters without `environment_repair` open a manual repair shell and record `needs-attention`. After manual repair, repeating `beam` reruns setup in the same sandbox. Authentication and permissions remain under the agent's normal controls. A missing agent executable blocks upload; a running but unauthenticated agent may require user input.

Older saved uploads receive the new launcher after upload and before restore, using the saved launcher variables and prerequisite metadata. The workspace snapshot and sandbox identity are preserved. Already-started transfers keep their existing launcher.

Beam prints progress before allocation, remote preparation, upload, restoration, setup, return packing, download, local apply, and cleanup.

The launcher records `preparing`, `repairing`, `needs-attention`, `running`, and `stopped`. Upload waits briefly for readiness. Long setup remains visible through `beam status`, `beam logs`, and `beam attach`.

“Running” describes the remote process. It does not prove authentication, task progress, or task completion. Without task evidence, status reports an unknown task state and recommends `beam status --watch --notify`.

Each agent adapter owns discovery, session files, generated settings, authentication variables, required tools, startup, and observation decoding. The registry currently exposes `claude` and `shell`. Codex is not implemented. Shared transfer and status code use the adapter contract without client-specific paths or terminal patterns.

Capabilities describe session transfer, structured events, terminal heuristics, and agent reports. The Claude adapter supports session transfer and terminal heuristics. It does not yet consume structured client events. The launcher supplies an explicit report command for all agents.

Normalized observations include process start, activity, input needed, input resolved, process exit, completion reported, failure reported, and unknown. Each observation identifies its source. Status keeps process state separate from task state. Within the current process run, the latest observation from each source applies. Structured client events take priority over agent reports, which take priority over terminal heuristics. A new process start clears earlier task reports. Completion reports do not prove correctness.

The Claude fallback checks the final nonempty footer in the visible tmux pane, including a footer wrapped across two lines. It recognizes known English confirmation and permission footers. A match displays `possible-input`, with `input_request.source` set to `terminal-heuristic`. The next action is `beam attach`. Pane content stays remote.

A disappearing footer clears only the heuristic. It does not prove activity or clear a stronger input report. A failed capture supplies an unavailable observation. Unknown dialogs and localized footers remain undetected. Beam never submits a response or changes permission policy. Shell sessions have no live probe. Return phases and retained-transfer guidance take priority over task evidence.

The watcher renders and notifies from the same collected snapshot. Repeated observations do not trigger another notification. An unavailable poll does not reset notification history. A confirmed resolution allows a later input request to notify again.

## Providers

| Provider | Allocation | Execution and transfer | Cleanup |
|---|---|---|---|
| Docker | Named and labeled container | Docker command-line tools | Check label, remove container |
| Docker over SSH | Same on the SSH host | SSH plus Docker tools | Same ownership check |
| SSH | Existing host | SSH standard streams | Check directory ownership markers |
| Daytona | New sandbox or named snapshot | API key, curl, OpenSSH | Delete owned sandbox |
| Steel | New computer or checkpoint restore | Steel preview CLI | Delete computer |

Daytona uses its REST API for allocation, state, startup, SSH tokens, and deletion. Commands and binary archives use OpenSSH. The transport preserves remote exit codes and supports interactive terminals. A private temporary header file supplies the API key. Command arguments do not contain the API key.

Allocation syncs its marker file and parent directory before the API request. It saves the response before returning the ID. An ambiguous response requires explicit recovery. Recovery and deletion verify the `beam.session` label.

Beam requests a root-owned sandbox and runs sandbox commands as root. If the SSH gateway uses a different user, Beam requires passwordless sudo. Beam installs base tools and the selected agent before upload. Custom targets use `daytona:SNAPSHOT`. The timeout sets an inactivity auto-stop interval, rounded up to minutes. Auto-deletion and the wall-clock time limit are disabled.

Beam starts stopped or archived sandboxes before attachment or return. It waits for snapshot preparation, stopping, archiving, and other temporary states. Startup and deletion polling each have a three-minute timeout. An API request already in progress can extend that interval. Deletion completes only when the sandbox is absent or reports a deleted state. Failed or incomplete cleanup keeps the transfer open for retry. A stopped process requires restart. Paused virtual machines require manual resume.

Steel's command SSH transport does not reliably expose the remote exit code. Beam writes an exit receipt and reads it through the execution API. File downloads stream through SSH. Interactive attachment uses the login-shell hook installed by the bootstrap script.

The project path stays the same on both sides. Docker, Steel, and Daytona use the source HOME path. SSH uses an isolated HOME for agent files.

## Retention and removal

`beam down --keep` records `retained`. The sandbox remains listed and can be inspected or removed through Beam. Returning again removes the retained sandbox without importing subsequent edits. Command help, return output, and status explain that further sandbox edits will not return through `beam down`.

`beam kill` removes owned resources without returning additional work. `beam forget --yes` only closes the local record; the user must remove remote resources separately.

Closed receipts retain recovery data. Outgoing archives are removed after the transfer closes. Users can remove recovery worktrees and closed receipt directories after verification. No automatic pruning is implemented. Acknowledging recovery does not prune it.

## Validation

Offline Daytona tests cover allocation markers, interrupted allocation, ownership, snapshot preparation, stop/archive transitions, delayed deletion, and cleanup retry after API failure. The optional Daytona integration test checks a real shell round trip and binary file transfer. Authenticated Claude continuation on Daytona requires a separate smoke test.

Local tests cover snapshot round trips, ownership checks, locking, file merging, archive validation, configuration, and command errors. Docker tests cover normal transfers, setup failure and retry, shell workspaces, returning extras, retained sandboxes, conflict recovery, and retry after local apply.

Steel integration tests allocate a real computer and remove it afterward. They exercise transport and simulated agent changes. Authenticated Claude continuation requires a separate smoke test with valid credentials and a real transcript. Docker tests also cover reviewed returns, preserved staging during merges, extra-file undo, stale plans, recovery acknowledgment, verification failure, task reports, notifications, blocked first-run notices, and setup reuse/invalidation. Adapter tests cover wrapped footers, stale or quoted prompt text, unsupported probes, evidence precedence, and notification transitions. A second test adapter uses its own session layout and event format through discovery, launch, observation, and return.

## Saved return plans and undo

`beam down --review` stops the remote session and downloads the immutable return package. The transfer stays `downloaded`. It saves the remote worktree, a versioned `return-plan.json`, and `return-files.json`. It does not apply Git changes, extras, or agent files. Git objects and private transfer records may change.

The plan records local and remote snapshots, changed paths, conflicts, and a proposed target snapshot when supported. Automatic combination treats each path's index and worktree entries together. Different edits to the same path require manual integration. It supports separate additions, deletions, binary files, symlinks, and executable modes without flattening staging. File/directory collisions are rejected. Combining diverging commits or branches is not implemented.

A proposed combined result has an inspection worktree. Its edits are not applied by `beam review --apply`. `beam review --open` opens the saved remote worktree for manual inspection and integration. `--diff` shows the sent-to-remote worktree diff without external diff programs. `--json` includes the plan, auxiliary files, task record, events, and timings.

Before applying, Beam checks the reviewed local Git snapshot and auxiliary file fingerprints. A changed Git snapshot rebuilds the plan and stops. `beam review --refresh` rebuilds an unapplied plan, including auxiliary files. It does not change the fixed remote snapshot. After apply starts, unexpected local changes stop the retry and leave saved copies for recovery.

Before mutation, Beam saves the local Git state in `before-return`, auxiliary file backups, an undo record, and an apply marker. Retries recognize the proposed Git state and already-returned auxiliary files. A partial Git checkout that does not match a known snapshot stops for recovery rather than guessing which changes belong to the user.

`beam undo` checks all recorded auxiliary paths before changing Git. It restores the previous Git state and auxiliary files only when current state matches the return or an already-restored state. Later incompatible edits cause failure and preserve both versions. Undo is repeatable, keeps returned work, and does not recreate or remove a sandbox. It restores Git state and file contents/modes; it does not undo external actions performed by the agent or restore running processes.

Review and undo default to the latest receipt with a saved return plan for the project. `--transfer ID` selects another receipt. Legacy receipts without saved plans remain inspectable through their existing recovery paths.

## Task records, evidence, and reuse

The optional `[task]` section supplies objective, completion criteria, last verified result, next action, and constraints. The preview, saved task record, and handoff retain these user-provided facts. Automatic transcript summarization is not implemented.

`[sandbox] verify` runs shell commands after setup in the session directory. A failed command starts environment repair for capable agents and manual recovery for shell transfers. Authentication and task completion remain unverified by process status.

The launcher appends timestamped events for setup, verification, agent startup, and exit. Command events record exit status and elapsed seconds. The `BEAM_REPORT` script accepts explicit `working`, `waiting`, `finished`, and `failed` reports with bounded single-line evidence. Status labels these as agent reports. Beam does not infer completion from inactivity or successful process exit.

`beam status --watch` polls evidence and prints changed status. `--notify` rings the terminal bell once for each newly observed attention, completion, failure, or exit event during that watch. Existing events do not notify on startup. A live terminal input request does notify on the first observation, even on startup. Repeated observations do not notify again until the prompt kind changes or the prompt clears. Failed polls do not count as clearing a prompt. Polling stops after `--count`, when the transfer closes, or when the user interrupts it. It does not install a background service or send external notifications.

Receipts contain diagnostic timings between saved phases. These include network operations and user waiting time. They are not CPU execution measurements. Return downloads retain the last 100 remote events for later inspection.

Optional `reuse_setup` requires configured verification and applies only within one sandbox. The fingerprint includes launcher content, forwarded environment, current commit and workspace tree, tool locations and version output, and explicit `setup_inputs`. Setup success plus verification success saves a fingerprint of the resulting environment inputs. Checks always rerun. Changed/missing inputs or verification failure disable reuse. Ignored dependencies are validated by the configured checks; ignored setup inputs must be listed explicitly. No artifacts are shared across projects or newly allocated sandboxes.

`beam restart` restarts only a stopped session or one needing attention. It retains the original task and configuration in the launcher. It cannot terminate a running session or restart a returning or retained transfer.

## Future work

These capabilities are not implemented or promised by the current commands:

- Non-Git workspaces, submodules, and separate Git LFS object transfer.
- Other coding agents and additional cloud providers.
- Beam-managed installation of complete project toolchains. Agents can attempt repair using the supplied context.
- Running service migration or live bidirectional synchronization.
- Automatic pruning of old recovery receipts.
