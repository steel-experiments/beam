# ABOUTME: Claude-only fallback: recognize visible input footers without sending input.
if ! screen=$(tmux capture-pane -p -t "$T" 2>/dev/null); then
  echo '{"kind":"unknown","source":"unavailable","detail":"Terminal input state could not be checked"}'
  exit 0
fi
footer=$(printf '%s\n' "$screen" | awk '
  NF { gsub(/^[[:space:]]+|[[:space:]]+$/, ""); previous=last; last=tolower($0) }
  END {
    confirmation="^(press )?enter to (continue|confirm)( · | • | - | or )esc(ape)? to cancel$"
    permission="^esc(ape)? to cancel( · | • | - )tab to amend$"
    if (last ~ confirmation || (previous " " last) ~ confirmation) print "confirmation"
    else if (last ~ permission || (previous " " last) ~ permission) print "permission"
  }')
case "$footer" in
  confirmation|permission) printf '{"kind":"input-needed","source":"terminal-heuristic","detail":"%s"}\n' "$footer" ;;
  *) echo '{"kind":"input-resolved","source":"terminal-heuristic","detail":"No recognized input footer is visible"}' ;;
esac
