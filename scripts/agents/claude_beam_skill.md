---
name: beam
description: Return this beamed session to the user's local machine with `beam down`. Use only when the user explicitly asks to beam down, return, send the work home, or end the sandbox session. Do not use when a task is only finished.
---

# beam in this sandbox

Beam moved this session from the user's local machine into this sandbox. The work goes back with `beam down`. Run it only when the user asks for it.

## Return the work

1. Make sure all work is in the project directory. Beam returns the Git state (commits, staged and unstaged changes), this conversation, and the configured extra files. It does not return other files or installed packages.
2. Tell the user what is complete and what is not. If the handoff enables the PR workflow, attempt a final push first. Report the PR URL, push failures, and remaining uncommitted work. Do not make a rushed commit to return.
3. Run `beam down`.

`beam down` writes the return request and returns immediately. After 5 seconds, beam stops this session and packs the work. Write your last reply before that time. Edits after the pack do not return.

4. Tell the user to run `beam down` on their local machine. If `beam down --wait` already runs there, the work goes home automatically.

## Rules

- Do not run `beam down` because the task looks complete. To report completion, run `sh "$BEAM_REPORT" finished "brief evidence"`. The user decides when the work goes home.
- Only the local machine applies the returned work, and it never overwrites local changes.
- Other beam commands, such as `beam status`, run on the local machine. In this sandbox, beam has only `beam down`, `beam attach`, and `beam ls`.
