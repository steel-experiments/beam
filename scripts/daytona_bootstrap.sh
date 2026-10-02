# ABOUTME: Installs transfer prerequisites in a fresh root-owned Daytona sandbox.
set -eu
need=""
for t in git gh tmux curl tar gzip bash; do command -v "$t" >/dev/null 2>&1 || need="$need $t"; done
if [ -n "$need" ]; then
  echo "beam: installing$need"
  command -v apt-get >/dev/null 2>&1 || {
    echo 'beam: use a Debian/Ubuntu snapshot with git, tmux, curl, tar, gzip, and bash' >&2
    exit 4
  }
  apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq git gh tmux curl ca-certificates tar gzip bash >/dev/null
fi
