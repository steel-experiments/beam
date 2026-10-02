---
name: beam
description: Move this Claude Code session to a sandbox with `beam`, so that it continues there. Use when the user asks to beam up, beam this session, or move or continue the work in a sandbox, Steel, Docker, or Daytona.
---

# beam up from this session

Beam moves this session to a sandbox: the Git state (commits, staged and unstaged changes), this conversation, and the configured extra files. The session continues in the sandbox. The user can then close this local session.

## Move the session

1. Find the target. If the user names one, use it: `steel`, `docker`, `daytona`, `docker+ssh://HOST`, `ssh://HOST`, `steel:CHECKPOINT`, or `daytona:SNAPSHOT`. Otherwise run `beam default`. If it shows no personal default, ask the user for a target.
2. Find the permission mode. If the user wants the sandbox agent to work without them, use `bypassPermissions`. It is safe only because the sandbox is separate from this machine. If the user wants approval for commands, use `acceptEdits`. If the user does not say, ask. To use the `[agent] permission_mode` value from `beam.toml`, do not give the option.
3. Write the next step in one sentence, from this conversation. The sandbox agent starts with it at once. Include what is done and what to do next.
4. Run this command with a Bash timeout of 600000 ms:

   ```sh
   beam -y -d --force --session "${CLAUDE_SESSION_ID}" --to TARGET --permission-mode MODE --continue "NEXT STEP"
   ```

   - `-y` does not ask questions, and `-d` does not attach this terminal.
   - If the user requests commits, pushes, and a draft pull request, add `--pr`. It also forwards GitHub authentication.
   - If the user only requests GitHub access, add `--github-auth`.
   - These options use `GH_TOKEN`, `GITHUB_TOKEN`, or the local `gh` login. Do not print credentials. A successful push preserves committed code, but not uncommitted work or this conversation.
   - `--force` is necessary because this session still runs in the project.
   - If beam cannot find the session, run the command again without `--session`. Then beam uses the session that changed last, which is this one.
   - If the command stops before it completes, run it again. Beam continues the same transfer.
   - If a check fails, tell the user the error. Do not change the project to make it pass.
5. Write a short last reply:
   - Where the session runs now.
   - `beam attach` opens it, `beam status` shows its state, and `beam down` brings the work home.
   - The user can close this session now (`/exit`). If the user writes more messages here, they stay local and do not go to the sandbox.

## Rules

- Move the session only when the user asks for it.
- After `beam` starts, do not edit files and do not run other tools. The sandbox continues the work. Local edits after the send do not go to the sandbox.
- Write only one reply after `beam` completes. Beam replaces this local conversation with the sandbox conversation on `beam down`, but only when no user message follows the move.
