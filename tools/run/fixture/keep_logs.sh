#!/bin/bash
# Copy the failed fixture's logs to the gate's directory in the lease keep.
valid_seg() {
  local LC_ALL=C
  case "$1" in
    ""|"."|".."|-*|*/*|*\\*|*[[:space:][:cntrl:]]*) return 1 ;;
  esac
}
if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: keep_logs.sh <src-dir> <dest-name> [subdir]" >&2
  exit 2
fi
valid_seg "$2" || { echo "invalid keep destination: $2" >&2; exit 2; }
if [ "$#" -eq 3 ]; then
  valid_seg "$3" || { echo "invalid keep subdirectory: $3" >&2; exit 2; }
fi
[ -n "${DEVHUB_KEEP:-}" ] && [ -d "$1" ] && [ ! -L "$1" ] || exit 0
case "$DEVHUB_KEEP" in
  /*) keep_root=$DEVHUB_KEEP ;;
  *) keep_root=$PWD/$DEVHUB_KEEP ;;
esac
to=$keep_root/$2
[ ! -L "$to" ] || exit 2
if [ "$#" -eq 3 ]; then
  to=$to/$3
  [ ! -L "$to" ] || exit 2
fi
cd -- "$1" || exit 1
# The first branch includes every top-level file. Nested matches exclude app homes and caches.
find . -type f \( ! -path './*/*' -o \
  \( -name '*.log' -o -name log -o -name 'log.*' -o -name '*.out' -o -name '*.fail' \
     -o -name report -o -name 'report.*' -o -name stall -o -name stuck -o -name reqs \
     -o -path '*/captures/*' \) \
   ! -path '*/home/*' ! -path '*/run/*' ! -path '*/run-prep/*' ! -path '*/runhome/*' \
   ! -path '*/cache/*' ! -path '*/target/*' \) -print0 |
while IFS= read -r -d '' f; do
  mkdir -p -- "$to/${f%/*}" && cp -p -- "$f" "$to/$f" || exit 1
done
