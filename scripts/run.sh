# ABOUTME: Runs setup and the agent, recording readiness and persistent logs.
set -u
umask 077
# A mkdir claim prevents duplicate agents when an upload is retried.
mkdir "$S/run-lock" 2>/dev/null || exit 0
trap 'rmdir "$S/run-lock" 2>/dev/null || true' EXIT
printf '%s\n' started > "$S/started"
printf '%s\n' preparing > "$S/phase"
if [ -f "$S/env" ]; then
  set -a
  . "$S/env"
  set +a
fi
export HOME="$H"
cd "$P" || exit 3
report=""
failed=0
if [ -n "$SETUP" ]; then
  while IFS= read -r cmd; do
    [ -n "$cmd" ] || continue
    printf '\nbeam setup: %s\n' "$cmd"
    sh -c "$cmd" </dev/null > "$S/setup-last.log" 2>&1
    code=$?
    cat "$S/setup-last.log"
    cat "$S/setup-last.log" >> "$S/setup.log"
    if [ "$code" -eq 0 ]; then
      report="$report
- Setup command succeeded: $cmd"
    else
      report="$report
- Setup command FAILED (exit $code): $cmd"
      failed=1
      break
    fi
  done <<SETUP_EOF
$SETUP
SETUP_EOF
fi
msg="$HANDOFF_HEAD$report
$HANDOFF_TAIL"
printf '%s\n' "$msg" > "$S/handoff.txt"
if [ "$failed" -ne 0 ]; then
  printf '%s\n' needs-attention > "$S/phase"
  echo 'Setup failed. Fix the environment here, then run beam again locally to retry setup.'
  # Keep a repair shell available. The phase remains needs-attention.
  sh -i
  exit 1
fi
printf '%s\n' running > "$S/phase"
resume "$msg"
code=$?
printf '%s\n' "$code" > "$S/agent.exit"
printf '%s\n' stopped > "$S/phase"
