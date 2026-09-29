# ABOUTME: Captures the full git state of a worktree as commits: HEAD, index, and worktree.
# ABOUTME: Usage: sh snapshot.sh <repo> <ref> <bundle-out|-> [<exclude-commit>]. Prints key=value lines.
set -eu
repo=$1 ref=$2 out=$3 exclude=${4:-}
cd "$repo"
export GIT_AUTHOR_NAME=beam GIT_AUTHOR_EMAIL=beam@localhost
export GIT_COMMITTER_NAME=beam GIT_COMMITTER_EMAIL=beam@localhost
head=$(git rev-parse -q --verify HEAD) || { echo "beam: the repository has no commits" >&2; exit 3; }
branch=$(git symbolic-ref -q --short HEAD || true)
idx_tree=$(git write-tree)
idx_commit=$(git commit-tree --no-gpg-sign "$idx_tree" -p "$head" -m "beam index $$")
tmp_index=$(mktemp "${TMPDIR:-/tmp}/beam-index.XXXXXX")
trap 'rm -f "$tmp_index"' EXIT
if [ -f "$(git rev-parse --git-path index)" ]; then
  cp "$(git rev-parse --git-path index)" "$tmp_index"
else
  rm -f "$tmp_index"
  GIT_INDEX_FILE=$tmp_index git read-tree "$idx_tree"
fi
GIT_INDEX_FILE=$tmp_index git add -A
wt_tree=$(GIT_INDEX_FILE=$tmp_index git write-tree)
# The worktree commit has the same shape as a stash entry, so "git stash apply" can use it.
wt_commit=$(git commit-tree --no-gpg-sign "$wt_tree" -p "$head" -p "$idx_commit" -m "beam worktree $$")
git update-ref "$ref" "$wt_commit"
if [ "$out" != "-" ]; then
  if [ -n "$exclude" ]; then
    git bundle create -q "$out" "$ref" "^$exclude"
  else
    git bundle create -q "$out" "$ref"
  fi
fi
printf 'head=%s\nbranch=%s\nidx_tree=%s\nwt_tree=%s\nwt_commit=%s\n' \
  "$head" "$branch" "$idx_tree" "$wt_tree" "$wt_commit"
