# ABOUTME: Configures GitHub credentials only in the transferred repository.
set -eu
cd "$P"
git remote set-url origin "$GITHUB_ORIGIN"
# An empty helper clears inherited helpers. Tokens stay in the environment, not Git config.
git config --local --replace-all credential.https://github.com.helper ''
git config --local --add credential.https://github.com.helper '!gh auth git-credential'
if [ -n "$TASK_BRANCH" ] && [ ! -f "$S/task-branch-created" ]; then
  if [ "$(git symbolic-ref -q --short HEAD || true)" != "$TASK_BRANCH" ]; then
    git switch -q -c "$TASK_BRANCH"
  fi
  printf '%s\n' "$TASK_BRANCH" > "$S/task-branch-created"
fi
