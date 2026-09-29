# ABOUTME: Beam-owned prerequisites, setup, and verification; callable by the repair agent.
set -u
umask 077
mkdir "$S/check-lock" 2>/dev/null || { echo 'Beam environment checks are already running' >&2; exit 2; }
trap 'rmdir "$S/check-lock" 2>/dev/null || true' EXIT
if [ -f "$S/env" ]; then set -a; . "$S/env"; set +a; fi
export HOME="$H"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.npm-global/bin:$PATH:/usr/local/bin"
cd "$P" || exit 3
event() { printf '%s\t%s\t%s\n' "$(date +%s)" "$1" "$2" >> "$S/events.tsv"; }
event setup-started ''
rm -f "$S/prerequisites-last.log" "$S/setup-last.log" "$S/verify-last.log"
report=""
failed=0
start=$(date +%s)
sh -c "$PREREQUISITES" </dev/null > "$S/prerequisites-last.log" 2>&1
code=$?
cat "$S/prerequisites-last.log"
cat "$S/prerequisites-last.log" >> "$S/setup.log"
event prerequisites-checked "exit=$code elapsed=$(( $(date +%s) - start ))s"
if [ "$code" -ne 0 ]; then
  failed=1
  report="- Project prerequisites FAILED (exit $code)."
  rm -f "$S/setup-key"
fi
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
if [ "$failed" -ne 0 ]; then
  :
elif [ -n "$key" ] && [ "$cached" = "$key" ]; then
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

{
  printf '%s\n' "$report"
  if [ "$failed" -ne 0 ]; then
    for log in prerequisites-last.log setup-last.log verify-last.log; do
      if [ -f "$S/$log" ]; then
        printf '\n%s (last 80 lines):\n' "$log"
        tail -n 80 "$S/$log"
      fi
    done
  fi
} > "$S/check-report.txt"
if [ "$failed" -ne 0 ]; then
  rm -f "$S/setup-key"
  if [ "$ENVIRONMENT_REPAIR" = yes ]; then phase=repairing; else phase=needs-attention; fi
  printf '%s\n' "$phase" > "$S/phase"
  event "$phase" 'environment checks failed'
  exit 1
fi
# A successful check is the only path from repair to running.
printf '%s\n' running > "$S/phase"
event environment-checks-passed 'setup passed; see verification events for project readiness'
