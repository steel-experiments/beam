# Beam implementation contract

This document describes implemented behavior. Future ideas live in the final section.

## Product scope

Beam moves a Git workspace between a local machine and a sandbox. A Claude Code session is optional. The default agent mode resumes an existing session or opens a shell when none exists.

The transfer includes the current checkout's reachable history, index, worktree, selected ignored files, and agent files. It recreates dependencies through setup commands. It does not copy process memory, running services, databases, or installed toolchains.

Supported source platforms are Linux and macOS. The repository needs at least one commit. Submodules are rejected before transfer. Detected Git LFS configuration is rejected because its separate object store does not transfer. Windows is not supported.

## User workflow

`beam` builds a plan, checks prerequisites, asks for confirmation, and sends the workspace. Interactive mode attaches after startup. `--detach` returns to the local shell.

When no target is configured, interactive mode asks for one and saves it as a personal default. Noninteractive commands must supply a target or configured default. Target precedence is `--to`, project `[beam] to`, then personal `to`.

Claude sessions are discovered in the invocation directory, with a project-root fallback for automatic selection. Interactive mode offers a choice when several sessions exist. Noninteractive mode uses the latest modified transcript. Explicit session selection fails if the session is absent.

The plan shows the selected session, destination, changed paths, complete-history transfer, extras policies, environment variable names, setup commands, and compatibility warnings. Values of environment variables are not printed.

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

Docker allocation uses a deterministic container name and transfer label. An existing container must have the expected label. Steel allocation writes its response to a durable receipt. If the response is ambiguous, the user identifies the computer with `--recover-sandbox ID`; Beam does not allocate a duplicate automatically.

SSH preparation claims new directories with ownership markers. Existing unowned paths are rejected. Cleanup checks markers before removing paths. Agent files use a private home under the transfer stage on SSH. Shared user files are not overwritten or removed.

## Git snapshots

`scripts/snapshot.sh` runs both locally and remotely. It creates an index commit and a worktree commit using a temporary Git index. The worktree commit has the original HEAD and index commit as parents.

Upload bundles all objects reachable from the worktree commit. Return bundles exclude objects reachable from the sent snapshot. The branch, HEAD, index tree, and worktree tree are recorded separately.

Restore reconstructs the checkout, worktree, and index. Staged and unstaged changes remain distinct. Git preserves executable bits, not all filesystem permission bits. Symlinks in Git preserve their link targets; external targets do not transfer.

Before applying returned Git changes, Beam saves a separate recovery worktree. It compares the current local snapshot with the sent snapshot. If local state changed, remote work stays in the recovery worktree. If local state already matches the returned state, a retry skips the apply.

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

Agent files use the same content comparison. Agent-file deletions are not propagated. Their return scope is the project session directory and Claude file history.

A return package is immutable after successful remote creation. Downloads stream to a private temporary file and rename after success. Extraction streams regular files to disk. Links, traversal, duplicate paths, and unexpected archive entries are rejected.

The `applied` phase is saved before sandbox cleanup. Cleanup retries do not reapply files. A conflict return reports a nonzero exit status after preserving recovery data.

## Setup and readiness

Lockfile detection selects one command per ecosystem. Each rule contains its lockfile, ecosystem, command, and required tool. Explicit setup commands replace detection.

Supported numeric toolchain pins are checked against the destination. Dynamic aliases and unsupported tools produce warnings. Beam does not install arbitrary toolchains. Custom images and Steel checkpoints supply them.

Remote setup runs before the agent. It records output in a persistent setup log. A failed setup command stops the sequence and opens a repair shell. The reported state is `needs-attention`. After repair, repeating `beam` reruns setup in the same sandbox.

The launcher records `preparing`, `needs-attention`, `running`, and `stopped`. Upload waits briefly for readiness. Long setup remains visible through `beam status`, `beam logs`, and `beam attach`.

“Running” describes the remote process. It does not prove authentication or task completion. A stopped session can be inspected through a repair shell.

## Providers

| Provider | Allocation | Execution and transfer | Cleanup |
|---|---|---|---|
| Docker | Named and labeled container | Docker command-line tools | Check label, remove container |
| Docker over SSH | Same on the SSH host | SSH plus Docker tools | Same ownership check |
| SSH | Existing host | SSH standard streams | Check directory ownership markers |
| Steel | New computer or checkpoint restore | Steel preview CLI | Delete computer |

Steel's command SSH transport does not reliably expose the remote exit code. Beam writes an exit receipt and reads it through the execution API. File downloads stream through SSH. Interactive attachment uses the login-shell hook installed by the bootstrap script.

The project path stays the same on both sides. Docker and Steel use the source HOME path. SSH uses an isolated HOME for agent files.

## Retention and removal

`beam down --keep` records `retained`. The sandbox remains listed and can be inspected or removed through Beam. Returning again removes the retained sandbox without importing subsequent edits.

`beam kill` removes owned resources without returning additional work. `beam forget --yes` only closes the local record; the user must remove remote resources separately.

Closed receipts retain recovery data. Outgoing archives are removed after the transfer closes. Users can remove recovery worktrees and closed receipt directories after verification. No automatic pruning is implemented.

## Validation

Local tests cover snapshot round trips, ownership checks, locking, file merging, archive validation, configuration, and command errors. Docker tests cover normal transfers, setup failure and retry, shell workspaces, returning extras, retained sandboxes, conflict recovery, and retry after local apply.

Steel integration tests allocate a real computer and remove it afterward. They exercise transport and simulated agent changes. Authenticated Claude continuation requires a separate smoke test with valid credentials and a real transcript.

## Future work

These capabilities are not implemented or promised by the current commands:

- Non-Git workspaces, submodules, and separate Git LFS object transfer.
- Other coding agents and additional cloud providers.
- Automatic installation of complete project toolchains.
- Running service migration or live bidirectional synchronization.
- Automatic pruning of old recovery receipts.
