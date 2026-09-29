# ABOUTME: Rebuilds the project in the sandbox from an unpacked snapshot in $S.
# ABOUTME: Needs these variables: S P H REF BRANCH HEAD_SHA IDX_TREE WT_TREE ORIGIN GIT_NAME GIT_EMAIL.
set -eu
if [ -e "$P/.git" ]; then
  echo "beam: $P already has a git repository" >&2
  exit 3
fi
mkdir -p "$P"
cd "$P"
git -c init.defaultBranch=beam init -q
git fetch -q "$S/repo.bundle" "$REF:$REF"
if [ -n "$BRANCH" ]; then
  git checkout -q -B "$BRANCH" "$HEAD_SHA"
else
  git checkout -q --detach "$HEAD_SHA"
fi
git read-tree -m -u HEAD "$WT_TREE"
git read-tree "$IDX_TREE"
git update-index -q --refresh >/dev/null || true
if [ -n "$ORIGIN" ]; then git remote add origin "$ORIGIN"; fi
if [ -n "$GIT_NAME" ]; then git config user.name "$GIT_NAME"; fi
if [ -n "$GIT_EMAIL" ]; then git config user.email "$GIT_EMAIL"; fi
if [ -d "$S/extras" ]; then cp -R "$S/extras/." "$P/"; fi
mkdir -p "$H"
if [ -d "$S/home" ]; then cp -R "$S/home/." "$H/"; fi
# Files in defaults/ are copied only when the sandbox does not have them yet.
if [ -d "$S/defaults" ]; then
  cd "$S/defaults"
  find . -type f | while IFS= read -r f; do
    if [ ! -e "$H/$f" ]; then
      mkdir -p "$(dirname "$H/$f")"
      cp "$f" "$H/$f"
    fi
  done
fi
echo "beam: restore done"
