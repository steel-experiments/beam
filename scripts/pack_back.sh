# ABOUTME: Makes one immutable return package. Retries reuse it.
set -eu
# A pack that starts in the sandbox (remote beam down) and one from the local beam can overlap.
i=0
until mkdir "$S/pack-lock" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -gt 600 ]; then
    echo "beam: another pack holds $S/pack-lock. If no pack runs, remove that directory" >&2
    exit 3
  fi
  sleep 1
done
trap 'rmdir "$S/pack-lock" 2>/dev/null || true' EXIT
if [ -f "$S/back.tar.gz" ]; then echo 'beam: return package already saved'; exit 0; fi
mkdir -p "$S/back/extras"
# Publication evidence is read-only. Failures must not prevent workspace recovery.
# A failed probe keeps the lines that it wrote before it stopped.
if [ -f "$S/publication.sh" ]; then
  (
    export HOME="$H"
    export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.npm-global/bin:$PATH:/usr/local/bin"
    if [ -f "$S/env" ]; then set -a; . "$S/env"; set +a; fi
    cd "$P"
    sh "$S/publication.sh"
  ) > "$S/back/publication.txt" || printf 'published=unknown\n' >> "$S/back/publication.txt"
fi
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
publication=""
if [ -f "$S/back/publication.txt" ]; then publication=publication.txt; fi
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
  COPYFILE_DISABLE=1 tar czf "$S/back.tar.gz.partial" -C "$S/back" info repo.bundle extras $publication -C "$H" "$@"
else
  COPYFILE_DISABLE=1 tar czf "$S/back.tar.gz.partial" -C "$S/back" info repo.bundle extras $publication
fi
mv "$S/back.tar.gz.partial" "$S/back.tar.gz"
echo 'beam: pack done'
