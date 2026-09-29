# ABOUTME: Packs the sandbox state for "beam down": new git objects, git state, and agent files.
# ABOUTME: Needs these variables: S P H REF SENT AGENT_PATHS (paths relative to $H, one per line).
set -eu
rm -rf "$S/back" "$S/back.tar.gz"
mkdir -p "$S/back"
sh "$S/snapshot.sh" "$P" "$REF" "$S/back/repo.bundle" "$SENT" > "$S/back/info"
cd "$H"
set -f --
for p in $AGENT_PATHS; do
  if [ -e "$p" ]; then set -- "$@" "$p"; fi
done
COPYFILE_DISABLE=1 tar czf "$S/back.tar.gz" -C "$S/back" info repo.bundle -C "$H" "$@"
echo "beam: pack done"
