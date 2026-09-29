# ABOUTME: Runs setup and project checks, recording evidence and elapsed time.
set -u
umask 077
mkdir "$S/run-lock" 2>/dev/null || exit 0
trap 'rmdir "$S/run-lock" 2>/dev/null || true' EXIT
printf '%s\n' started > "$S/started"
printf '%s\n' preparing > "$S/phase"
event() { printf '%s\t%s\t%s\n' "$(date +%s)" "$1" "$2" >> "$S/events.tsv"; }
event setup-started ''
if [ -f "$S/env" ]; then set -a; . "$S/env"; set +a; fi
export HOME="$H"
export BEAM_REPORT="$S/report.sh"
cd "$P" || exit 3
report=""
failed=0
# Cache only within this sandbox. Include the launcher, forwarded environment, tool versions,
# and all workspace content. A changed input reruns setup. Verification always runs.
setup_key() {
  (
    git hash-object "$S/run.sh"
    if [ -f "$S/env" ]; then git hash-object "$S/env"; fi
    git rev-parse HEAD
    index=$(mktemp "$S/cache-index.XXXXXX") || exit 1
    rm -f "$index"
    trap 'rm -f "$index"' EXIT
    GIT_INDEX_FILE=$index git read-tree HEAD || exit 1
    GIT_INDEX_FILE=$index git add -A || exit 1
    GIT_INDEX_FILE=$index git write-tree || exit 1
    printf '%s\n' "$SETUP_INPUTS" | while IFS= read -r input; do
      [ -n "$input" ] || continue
      git hash-object -- "$input" || exit 1
    done || exit 1
    printf '%s\n' "$TOOLS" | while IFS= read -r tool; do
      [ -n "$tool" ] || continue
      command -v "$tool" || exit 1
      "$tool" --version 2>&1 || "$tool" -V 2>&1 || exit 1
    done || exit 1
  ) > "$S/setup-inputs" || return 1
  git hash-object "$S/setup-inputs"
}
key=""
if [ "$REUSE_SETUP" = yes ] && [ -n "$VERIFY" ]; then key=$(setup_key) || key=""; fi
cached=$(cat "$S/setup-key" 2>/dev/null || true)
if [ -n "$key" ] && [ "$cached" = "$key" ]; then
  event setup-reused 'matching inputs; project checks will run again'
  report="$report
- Setup reused in the same sandbox; inputs match."
else
  rm -f "$S/setup-key"
  if [ -n "$SETUP" ]; then
    while IFS= read -r cmd; do
      [ -n "$cmd" ] || continue
      printf '\nbeam setup: %s\n' "$cmd"
      start=$(date +%s)
      sh -c "$cmd" </dev/null > "$S/setup-last.log" 2>&1
      code=$?
      cat "$S/setup-last.log"
      cat "$S/setup-last.log" >> "$S/setup.log"
      event setup-command "exit=$code elapsed=$(( $(date +%s) - start ))s"
      if [ "$code" -eq 0 ]; then report="$report
- Setup command succeeded: $cmd"
      else report="$report
- Setup command FAILED (exit $code): $cmd"; failed=1; break; fi
    done <<SETUP_EOF
$SETUP
SETUP_EOF
  fi
fi
if [ "$failed" -eq 0 ]; then
  event setup-passed ''
  if [ -n "$VERIFY" ]; then
    while IFS= read -r cmd; do
      [ -n "$cmd" ] || continue
      printf '\nbeam verify: %s\n' "$cmd"
      start=$(date +%s)
      sh -c "$cmd" </dev/null > "$S/verify-last.log" 2>&1
      code=$?
      cat "$S/verify-last.log"
      cat "$S/verify-last.log" >> "$S/setup.log"
      event verification-command "exit=$code elapsed=$(( $(date +%s) - start ))s"
      if [ "$code" -eq 0 ]; then report="$report
- Project check passed: $cmd"
      else report="$report
- Project check FAILED (exit $code): $cmd"; failed=1; event verification-failed ''; break; fi
    done <<VERIFY_EOF
$VERIFY
VERIFY_EOF
    if [ "$failed" -eq 0 ]; then
      event verification-passed ''
      if [ -n "$key" ]; then
        key=$(setup_key) || key=""
        if [ -n "$key" ]; then printf '%s\n' "$key" > "$S/setup-key"; fi
      fi
    else rm -f "$S/setup-key"; fi
  else event verification-unconfigured 'project readiness is unverified'; fi
fi
msg="$HANDOFF_HEAD$report
$HANDOFF_TAIL"
printf '%s\n' "$msg" > "$S/handoff.txt"
if [ "$failed" -ne 0 ]; then
  event needs-attention 'setup or project check failed'
  printf '%s\n' needs-attention > "$S/phase"
  echo 'Setup failed or a project check failed. Fix the environment here, then run beam again locally to retry.'
  sh -i
  exit 1
fi
printf '%s\n' running > "$S/phase"
event agent-started 'authentication and task completion are unverified'
resume "$msg"
code=$?
printf '%s\n' "$code" > "$S/agent.exit"
printf '%s\n' stopped > "$S/phase"
event agent-exited "exit=$code"
