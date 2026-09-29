# ABOUTME: Install the Claude client when the provider supplies only base tools.
set -eu
export PATH="$HOME/.local/bin:$PATH"
if ! command -v claude >/dev/null 2>&1; then
  echo "beam: installing Claude Code"
  curl -fsSL https://claude.ai/install.sh | bash >/dev/null
fi
