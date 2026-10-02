# ABOUTME: Captures read-only publication evidence at the fixed return snapshot.
set -eu
# Count NUL-delimited status entries, including names with newlines.
dirty=$(git -c status.renames=false status --porcelain=v1 -z --untracked-files=all | tr -cd '\000' | wc -c | tr -d ' ')
printf 'dirty=%s\n' "$dirty"
branch=$(git symbolic-ref -q --short HEAD || true)
head=$(git rev-parse HEAD)
published=unknown
pr=unknown
# Bound remote probes. Without timeout, keep network evidence unknown rather than delay recovery.
if [ -n "$branch" ] && command -v timeout >/dev/null 2>&1; then
  if remote=$(timeout 20 git ls-remote --exit-code origin "refs/heads/$branch" 2>/dev/null); then
    tip=$(printf '%s\n' "$remote" | cut -f1)
    if [ "$tip" = "$head" ]; then published=yes; else published=no; fi
  else
    result=$?
    if [ "$result" -eq 2 ]; then published=no; fi
  fi
  if command -v gh >/dev/null 2>&1; then
    pr=$(timeout 20 gh pr view "$branch" --json url --jq .url 2>/dev/null || echo unknown)
  fi
fi
printf 'published=%s\npr=%s\n' "$published" "$pr"
