#!/bin/sh
# Renders every screen of the demo to PNG files. Usage: scripts/shots.sh <exe> <out dir> [views...]
exe="$1"; out="$2"; shift 2
export __COMPAT_LAYER=RunAsInvoker
views="$*"
[ -z "$views" ] && views="home home-quick scanning stopped results preview preview-text bigpreview preview-doc kurdish kurdish-text grid recover recover-blocked recover-running recover-done images imaging log settings welcome shortcuts about"
mkdir -p "$out"
for v in $views; do
  "$exe" --demo --screenshot "$out/$v.png" --view "$v" || echo "failed: $v"
done
"$exe" --demo --light --screenshot "$out/light-results.png" --view preview
"$exe" --demo --light --screenshot "$out/light-home.png" --view home
