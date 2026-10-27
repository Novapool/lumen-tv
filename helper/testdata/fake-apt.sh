#!/bin/sh
# Fake apt-get for tests: records argv and environment next to this script, prints progress.
# With -s (simulate) it prints Inst/Remv lines for the named packages plus the contents of
# sim.extra (if present) next to this script, so tests can make apt "want" more than allowed.
d=$(dirname "$0")
{ echo "ARGS:$*"; for a in "$@"; do echo "ARG:$a"; done; env | sort | sed 's/^/ENV:/'; echo "END"; } >> "$d/apt.log"
sim=0; verb=""; after=0
for a in "$@"; do
  [ "$a" = "-s" ] && sim=1
  case "$a" in install|remove|update) [ -z "$verb" ] && [ $after = 0 ] && verb=$a ;; esac
  [ "$a" = "--" ] && after=1
done
for a in "$@"; do
  case "$a" in
    slowpkg) [ $sim = 0 ] && sleep 3 ;;
    failpkg) echo "E: Unable to locate package failpkg" >&2; exit 100 ;;
  esac
done
echo "Reading package lists..."
if [ $sim = 1 ]; then
  after=0
  for a in "$@"; do
    if [ $after = 1 ]; then
      if [ "$verb" = remove ]; then echo "Remv $a [1.0]"; else echo "Inst $a (1.0 Test [arm64])"; fi
    fi
    [ "$a" = "--" ] && after=1
  done
  [ -f "$d/sim.extra" ] && cat "$d/sim.extra"
  exit 0
fi
printf 'Progress: 10%%\rProgress: 60%%\rProgress: 100%%\n'
echo "done $*" >&2
