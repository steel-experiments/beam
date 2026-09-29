# Live handoff test: 29 September 2026

The core round trip passed with an authenticated Claude Code session on a fresh Steel computer. Unattended startup did not pass: a Claude gateway notice required one manual acknowledgment.

## Environment

| Component | Tested configuration |
|---|---|
| Local client | Claude Code 2.1.280 on Linux x86_64 |
| Remote client | Claude Code 2.1.285 on a fresh Steel Debian computer |
| Authentication | Existing configured gateway, forwarded through environment variables |
| Model | The existing gateway configuration resolved the local request to `glm-5.3[1m]` |
| Project | Disposable Python login-redirect function with three unit tests |
| Initial Git state | One commit; staged and unstaged changes in `DESIGN.md` |
| Transfer | `beam --yes --detach --session ID` |

This exercises the real Claude Code client and its configured gateway. It does not establish compatibility with a direct Anthropic login. The test uses a detached client; it does not physically disconnect or power off the source machine.

## Procedure and results

1. Started a real local conversation. Claude reproduced one failing test and stopped without fixing it. A unique phrase existed only in the conversation.
2. Transferred the workspace and conversation to Steel. Setup installed Python. The environment check passed. The remote agent recalled the phrase without a second explanation of the task.
3. After the upload command exited, changed `NOTES.md` locally. The remote agent fixed `login.py`, wrote the remembered phrase to `handoff-proof.txt`, and reported completion. All three tests passed independently on the remote machine.
4. Ran `beam down --review`. The plan identified two remote paths and one local path, with no conflicts. Local code, staging, and the local note remained unchanged.
5. Ran `beam review --apply`. All three tests passed locally. The phrase matched exactly. The independent local note, original commit, index tree, and staged/unstaged contents of `DESIGN.md` were preserved. Beam removed the Steel computer.
6. Resumed the returned conversation locally without tools or session persistence. It recalled the exact phrase, the remote fix, and the successful test result.
7. Added a later local edit and attempted undo. Beam refused and preserved it. After removing that test edit, undo restored the pre-return code, index, transcript, and agent files. The original failing test returned, as expected. Repeating undo was harmless. The returned fix remained in the saved worktree.

## Observed timings

| Operation | Elapsed time |
|---|---:|
| Local conversation and failure diagnosis | 15.70 seconds |
| Cold allocation, setup, upload, and startup | 49.85 seconds |
| Agent startup to completion report | 80 seconds, including the notice pause |
| Stop remote session and download for review | 21.68 seconds |
| Apply and remove the remote computer | 1.39 seconds |
| Read-only local conversation resume | 4.00 seconds |
| Undo | 0.06 seconds |

These are one-run observations, not performance guarantees. Phase records attribute 36 seconds of the cold transfer to remote preparation, which includes installing base tools.

## Friction found

**A first-run notice blocked progress.** Claude displayed a gateway auto-mode billing notice with “Enter to continue.” The process remained alive and Beam reported `running`. Sending Enter allowed the agent to continue. The task needed no new explanation and no other interactive approval. This was one manual intervention, so the run does not qualify as fully unattended.

**The notice was not represented as an attention event.** `beam status` accurately reported process state, but it could not identify this client dialog. The agent later emitted a completion report, which status displayed correctly. A notification bell was not demonstrated in this smoke test because the watcher first observed the report after completion.

**File tracking produced a stale-change warning.** The remote Edit tool warned that `login.py` had changed since the earlier local read. The agent reread the file, confirmed its intended edit, and continued. No user action was needed.

**The test harness needed two CLI adjustments.** A model warning preceded the JSON result, so the harness parsed the result line. An empty tool-server configuration required `{"mcpServers":{}}`, rather than `{}`. These did not require changes to Beam.

## Follow-up priority

Make the first interactive action part of the handoff acceptance check. Report readiness conservatively until the agent performs a useful action. Investigate supported client events for first-run dialogs and permission prompts. Preserve user consent rather than automatically dismissing arbitrary dialogs.

The observed round-trip duration also warrants measuring the remote stop sequence separately from packing and download.

## Cleanup and evidence

The test computer was deleted. A final Steel listing contained only the pre-existing paused computer. Authentication settings in the disposable local home were removed after verification.

The private local fixture and receipt remain for inspection:

- Project: `/tmp/beam-real-claude-w7gy9hng/home/dev/login-app`
- Receipt: `/tmp/beam-real-claude-w7gy9hng/home/.beam/transfers/1790712902-116881442`
- Returned work: the receipt's `worktree` directory

The project is left in the successfully undone state. The saved remote worktree contains the working fix. No credentials or raw transcripts are included in this report.


## Retest after input detection fix

Repeated the handoff on a new Steel computer using the same authenticated conversation and the corrected Beam binary. The fresh remote client displayed the same gateway notice.

- `beam status --json` returned `phase: "waiting-for-input"`, `next_action: "beam attach"`, and `input_request: {"source":"terminal-footer","kind":"confirmation","next_action":"beam attach"}`.
- `beam status --watch --notify --count 2 --interval 1` rang exactly once across two polls. Status did not answer the notice or restart the agent.
- After the test operator pressed Enter, status returned `remote: "running"` and `input_request: null` on the next observation.
- The agent continued its original task, emitted working and finished reports, and passed all three tests on the remote machine.

The observed first-run dialog is now visible and actionable. It still needs a human response. The detector covers known English confirmation and permission footers, not every possible client dialog. Process startup output now states that task progress is unverified and recommends watching for input requests.

The retest also returned successfully: all three tests passed locally, undo restored the fixture, and the new Steel computer was deleted. Temporary authentication settings were removed again. The retest receipt is `1790713630-990460922` in the same private transfer directory.

Automated validation after the fix passed 45 local tests and 13 Docker integration tests, including the new blocked-notice regression. Formatting, Clippy, and whitespace checks passed.

## Retest after the agent adapter refactor

Repeated the authenticated round trip on a new Steel computer. The remote client was Claude Code 2.1.285. The test used the same gateway and conversation as the earlier runs.

- Startup reported a running process with unknown task state. The notice appeared after the agent attempted its first tool call.
- The visible notice produced `possible-input` with `terminal-heuristic` evidence. Two watcher polls produced exactly one bell. The test operator acknowledged the notice once.
- Claude continued the original task and emitted a completion report. Status returned `task.state: "completion-reported"` with source `agent-report`. The process remained running.
- All three project tests passed remotely and after return. The remembered phrase matched. The Git index, staged and unstaged design notes, and independent local notes were preserved.
- A local conversation resume recalled the phrase, the remote fix, and the three passing tests. Undo restored the fixture and preserved the returned work in the receipt.
- The test computer was removed. The final listing contained only the pre-existing paused computer. Temporary authentication settings were removed again.

Cold upload and startup took 51.44 seconds. Return took 23.27 seconds. Local conversation recall took 5.53 seconds. These are single-run observations.

The private receipt is `1790714794-637835652` in the same transfer directory. The run required one manual acknowledgment, so it does not demonstrate unattended execution.

Validation passed 52 local tests and 13 Docker integration tests. Formatting, Clippy, and whitespace checks passed. The local tests include a second adapter with a different file layout and event format. It exercises discovery, generated settings, launch, observations, and returned session files. It does not implement or validate Codex support.
