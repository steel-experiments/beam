# ABOUTME: Test image for the end-to-end test: the sandbox tools and a fake "claude" command.
# ABOUTME: The fake agent writes to the transcript and the worktree, then waits for Ctrl-C.
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends git tmux ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY <<'FAKE' /usr/local/bin/claude
#!/bin/sh
[ "$1" = "--resume" ] || { echo "fake claude: expected --resume" >&2; exit 2; }
id=$2
if [ -f .beam-test-input-prompt ]; then
  printf '\033[2J\033[HGateway notice\nEnter to continue · Esc to cancel\n'
  echo pending > /tmp/fake-claude-notice
  read -r reply
  printf '\033[2J\033[HTask resumed\n'
fi
enc=$(pwd | sed 's/[^A-Za-z0-9]/-/g')
printf '{"type":"user","message":"from sandbox"}\n' >> "$HOME/.claude/projects/$enc/$id.jsonl"
printf '%s\n' "$3" > /tmp/fake-claude-handoff
printf '%s\n' "${BEAM_E2E_VAR:-unset}" > /tmp/fake-claude-env
echo "work from sandbox" > sandbox-work.txt
git add sandbox-work.txt && git commit -q -m "agent commit"
echo "uncommitted line" >> README.md
echo ready > /tmp/fake-claude-ready
trap 'exit 0' INT
while :; do sleep 1; done
FAKE
RUN chmod 755 /usr/local/bin/claude
