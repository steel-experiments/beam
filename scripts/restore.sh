# ABOUTME: Rebuilds the project in the sandbox from an unpacked snapshot in $S.
# ABOUTME: Needs these variables: S P H REF BRANCH HEAD_SHA IDX_TREE WT_TREE ORIGIN GIT_NAME GIT_EMAIL.
set -eu
if [ -f "$S/restored" ]; then exit 0; fi
if [ -e "$P/.git" ] && [ ! -f "$S/restore-started" ]; then
  echo "beam: $P already has a git repository" >&2
  exit 3
fi
touch "$S/restore-started"
mkdir -p "$P"
cd "$P"
git -c init.defaultBranch=beam init -q
if [ -f "$P/.beam-owner" ]; then mv "$P/.beam-owner" "$P/.git/beam-owner"; fi
git fetch -q "$S/repo.bundle" "+$REF:$REF"
if [ -n "$BRANCH" ]; then
  git checkout -q -f -B "$BRANCH" "$HEAD_SHA"
else
  git checkout -q -f --detach "$HEAD_SHA"
fi
git read-tree --reset -u "$WT_TREE"
git read-tree "$IDX_TREE"
git update-index -q --refresh >/dev/null || true
if [ -n "$ORIGIN" ]; then git remote remove origin 2>/dev/null || true; git remote add origin "$ORIGIN"; fi
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
cd "$P"
printf '\n/.beam-owner\n/.beam/\n' >> .git/info/exclude
touch "$S/restored"
echo "beam: restore done"
