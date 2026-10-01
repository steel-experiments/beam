# ABOUTME: The beam command in a sandbox: open a transferred session, or pack its work for the return.
# ABOUTME: Needs ROOT (the .beam directory of the sandbox user). Only the local beam applies returned work.
set -u
usage() {
  cat <<'EOF'
beam in a sandbox:
  beam attach [ID]  open the agent terminal
  beam down [ID]    stop the agent and pack its work for the return
  beam ls           list the transfers in this sandbox
Then run beam down on your local machine to bring the work home.
Run all other commands, such as beam status, on your local machine.
EOF
}
event() { printf '%s\t%s\t%s\n' "$(date +%s)" "$2" "$3" >> "$1/events.tsv"; }
quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

# Transfers that this version of beam can return. Each one has a return script.
transfers() {
  for s in "$ROOT"/remote/*; do
    if [ -f "$s/return.sh" ]; then printf '%s\n' "$s"; fi
  done
}

state() {
  if [ -f "$1/return-ready" ]; then echo 'packed; run beam down locally'
  elif [ -f "$1/return-failed" ]; then echo 'pack failed'
  elif [ -f "$1/return-requested" ]; then echo 'packing'
  else cat "$1/phase" 2>/dev/null || echo preparing
  fi
}

# Print the stage of the chosen transfer. The agent session sets BEAM_STAGE.
pick() {
  if [ -n "${1:-}" ]; then
    if [ -f "$ROOT/remote/$1/return.sh" ]; then printf '%s\n' "$ROOT/remote/$1"; return 0; fi
    echo "beam: there is no transfer $1 in this sandbox. Run beam ls" >&2
    return 2
  fi
  if [ -n "${BEAM_STAGE:-}" ] && [ -f "$BEAM_STAGE/return.sh" ]; then
    printf '%s\n' "$BEAM_STAGE"
    return 0
  fi
  list=$(transfers)
  count=$(printf '%s' "$list" | grep -c .)
  if [ "$count" -eq 0 ]; then
    echo 'beam: there is no transfer in this sandbox' >&2
    return 2
  fi
  if [ "$count" -eq 1 ]; then printf '%s\n' "$list"; return 0; fi
  if [ ! -t 0 ]; then
    echo 'beam: this sandbox has more than one transfer. Give its ID; run beam ls to see them' >&2
    return 2
  fi
  i=0
  printf '%s\n' "$list" | while IFS= read -r s; do
    i=$((i + 1))
    printf '%s) %s  %s  %s\n' "$i" "${s##*/}" "$(cat "$s/project" 2>/dev/null)" "$(state "$s")" >&2
  done
  printf 'Transfer number: ' >&2
  read -r n || return 2
  case $n in '' | *[!0-9]*) n=0 ;; esac
  chosen=$(printf '%s\n' "$list" | sed -n "${n}p")
  if [ -z "$chosen" ]; then echo 'beam: no transfer has that number' >&2; return 2; fi
  printf '%s\n' "$chosen"
}

ls_transfers() {
  list=$(transfers)
  if [ -z "$list" ]; then echo 'no transfers in this sandbox'; return 0; fi
  printf '%s\n' "$list" | while IFS= read -r s; do
    printf '%s  %s  %s\n' "${s##*/}" "$(cat "$s/project" 2>/dev/null)" "$(state "$s")"
  done
}

attach() {
  S=$(pick "$@") || exit $?
  id=${S##*/}
  if [ -f "$S/return-requested" ]; then
    echo 'beam: the work of this transfer is packed for the return. Run beam down on your local machine' >&2
    exit 1
  fi
  if [ -n "${TMUX:-}" ]; then
    echo 'beam: this terminal is already in tmux. Detach with Ctrl-b d first' >&2
    exit 1
  fi
  for t in "beam-$id" "beam-$id-repair"; do
    if tmux has-session -t "=$t" 2>/dev/null; then
      # With two clients, the window follows the latest active one, not the smallest.
      tmux set-option -t "=$t" window-size latest >/dev/null 2>&1 || true
      exec tmux attach -t "=$t"
    fi
  done
  echo 'beam: the agent session has stopped. Run beam attach on your local machine for a repair shell' >&2
  exit 1
}

down() {
  S=$(pick "$@") || exit $?
  id=${S##*/}
  if [ -f "$S/return-ready" ]; then
    echo 'The work is already packed. On your local machine, run: beam down'
    exit 0
  fi
  if [ -f "$S/return-requested" ] && [ ! -f "$S/return-failed" ]; then
    echo 'The work is being packed. On your local machine, run: beam down'
    exit 0
  fi
  rm -f "$S/return-failed"
  date +%s > "$S/return-requested"
  inside=no
  if [ -n "${TMUX:-}" ]; then
    case $(tmux display-message -p '#S' 2>/dev/null) in
      "beam-$id" | "beam-$id-repair") inside=yes ;;
    esac
  fi
  if [ "$inside" = yes ]; then
    # This terminal stops with the agent. A separate tmux session does the work,
    # and the delay lets the agent finish its reply first.
    event "$S" return-requested 'beam down in the agent terminal'
    tmux kill-session -t "=beam-return-$id" 2>/dev/null || true
    if ! tmux new-session -d -s "beam-return-$id" \
      "sleep 5; sh $(quote "$S/return.sh") >> $(quote "$S/return.log") 2>&1"; then
      echo 'cannot start the pack session' > "$S/return-failed"
      echo 'beam: cannot start the pack session. Run beam down on your local machine' >&2
      exit 1
    fi
    echo 'Return requested. In 5 seconds the agent stops and its work is packed.'
  else
    event "$S" return-requested 'beam down in the sandbox'
    echo 'Stopping the agent and packing its work...'
    if ! sh "$S/return.sh"; then
      echo "beam: packing failed: $(cat "$S/return-failed" 2>/dev/null). Run beam down on your local machine to try again" >&2
      exit 1
    fi
    echo 'The work is packed.'
  fi
  echo 'On your local machine, run: beam down'
  echo 'Sandbox edits after the pack do not return.'
}

case "${1:-}" in
  attach) shift; attach "$@" ;;
  down | back) shift; down "$@" ;;
  ls) ls_transfers ;;
  '' | help | -h | --help) usage ;;
  *)
    echo "beam: $1 runs on your local machine, not in the sandbox" >&2
    usage >&2
    exit 2
    ;;
esac
