# ABOUTME: Records explicit task reports; no process state is treated as task completion.
set -eu
umask 077
stage=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case ${1:-} in working|waiting|finished|failed) ;; *) echo 'usage: sh "$BEAM_REPORT" working|waiting|finished|failed "evidence"' >&2; exit 2 ;; esac
# One line per event; limit detail and remove control characters.
detail=$(printf '%s' "${2:-}" | tr '\n\r\t' '   ' | tr -d '\000-\010\013\014\016-\037\177' | cut -c1-500)
printf '%s\tagent-reported-%s\t%s\n' "$(date +%s)" "$1" "$detail" >> "$stage/events.tsv"
