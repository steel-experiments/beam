# ABOUTME: Observe remote process state independently of the agent.
phase=$(cat "$S/phase" 2>/dev/null || echo preparing)
if [ "$phase" = needs-attention ]; then
  echo needs-attention
elif tmux has-session -t "$T" 2>/dev/null; then
  echo "$phase"
else
  echo "stopped $(cat "$S/agent.exit" 2>/dev/null || echo '?')"
fi
