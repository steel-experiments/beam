# ABOUTME: Makes a fresh Steel computer ready for beam: /proc, git, tmux, curl, Claude Code, attach hook.
# ABOUTME: Each step is skipped when it is already done, so a checkpoint of a ready computer starts fast.
set -eu
if [ ! -e /proc/self ]; then
  mount -t proc proc /proc
fi
need=""
for t in git tmux curl; do command -v "$t" >/dev/null 2>&1 || need="$need $t"; done
if [ -n "$need" ]; then
  echo "beam: installing$need"
  apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq git tmux ca-certificates curl >/dev/null
fi
export PATH="$HOME/.local/bin:$PATH"
if ! command -v claude >/dev/null 2>&1; then
  echo "beam: installing Claude Code"
  curl -fsSL https://claude.ai/install.sh | bash >/dev/null
fi
# "steel computer ssh -- CMD" has no terminal, but a login shell has one.
# beam attach writes the command to ~/.beam-attach, and the next login shell runs it.
if ! grep -q 'beam-attach' "$HOME/.bashrc" 2>/dev/null; then
  cat >> "$HOME/.bashrc" <<'HOOK'
if [ -t 0 ] && [ -z "${TMUX:-}" ] && [ -f "$HOME/.beam-attach" ]; then
  beam_cmd=$(cat "$HOME/.beam-attach"); rm -f "$HOME/.beam-attach"; exec sh -c "$beam_cmd"
fi
HOOK
fi
echo "beam: steel computer ready"
