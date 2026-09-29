# ABOUTME: Makes one immutable return package. Retries reuse it.
set -eu
if [ -f "$S/back.tar.gz" ]; then echo 'beam: return package already saved'; exit 0; fi
mkdir -p "$S/back/extras"
sh "$S/snapshot.sh" "$P" "$REF" "$S/back/repo.bundle" "$SENT" > "$S/back/info"
# Copy returning extras into staging. Do not follow symlinks.
printf '%s\n' "$EXTRAS" | while IFS= read -r p; do
  [ -n "$p" ] || continue
  if [ -L "$P/$p" ]; then echo "beam: returning extra is a symlink: $p" >&2; exit 3; fi
  if [ -e "$P/$p" ]; then
    mkdir -p "$S/back/extras/$(dirname "$p")"
    cp -RP "$P/$p" "$S/back/extras/$p"
  fi
done
cd "$H"
set -f
set --
# Newline alone separates paths; spaces in project names remain intact.
old_ifs=$IFS
IFS='
'
for p in $AGENT_PATHS; do
  if [ -e "$p" ]; then set -- "$@" "$p"; fi
done
IFS=$old_ifs
if [ "$#" -gt 0 ]; then
  COPYFILE_DISABLE=1 tar czf "$S/back.tar.gz.partial" -C "$S/back" info repo.bundle extras -C "$H" "$@"
else
  COPYFILE_DISABLE=1 tar czf "$S/back.tar.gz.partial" -C "$S/back" info repo.bundle extras
fi
mv "$S/back.tar.gz.partial" "$S/back.tar.gz"
echo 'beam: pack done'
