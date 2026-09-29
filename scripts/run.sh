# ABOUTME: Starts the agent in the sandbox. It runs inside tmux.
# ABOUTME: Needs these variables: S P H HANDOFF_HEAD HANDOFF_TAIL SETUP (one command per line) and a resume function.
set -u
if [ -f "$S/env" ]; then
  set -a
  . "$S/env"
  set +a
fi
export HOME="$H"
cd "$P"
report=""
if [ -n "$SETUP" ]; then
  while IFS= read -r cmd; do
    [ -n "$cmd" ] || continue
    printf '\n\033[1m▸ beam setup: %s\033[0m\n' "$cmd"
    sh -c "$cmd" </dev/null
    code=$?
    if [ "$code" -eq 0 ]; then
      report="$report
- Setup command succeeded: $cmd"
    else
      report="$report
- Setup command FAILED (exit $code): $cmd"
    fi
  done <<SETUP_EOF
$SETUP
SETUP_EOF
fi
msg="$HANDOFF_HEAD$report
$HANDOFF_TAIL"
printf '%s\n' "$msg" > "$S/handoff.txt"
resume "$msg"
echo "$?" > "$S/agent.exit"
