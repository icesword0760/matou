#!/usr/bin/env bash
# Contact sheet for the rendered film: one thumbnail every 15 seconds, each stamped with its own
# timestamp, tiled 6x5 into a single png. What it is for: spotting black frames, size jumps and
# sections that landed on the wrong screen without scrubbing the whole 5 minutes by hand.
#
#   qc/contact-sheet.sh [input.mp4] [output.png]
#
# The timestamp is drawn before `tile`, not after: after tiling there is one frame left and every
# cell would carry the same 0:00.
set -euo pipefail
cd "$(dirname "$0")/.."

IN="${1:-out/matou-launch-1080p.mp4}"
OUT="${2:-out/qc-sheet.png}"
EVERY="${EVERY:-15}"
FONT="${FONT:-/System/Library/Fonts/Helvetica.ttc}"

[ -f "$IN" ] || { echo "no such video: $IN (run npm run render first)" >&2; exit 1; }

ffmpeg -y -v error -i "$IN" -vf "\
fps=1/${EVERY},\
scale=480:-1,\
drawtext=fontfile=${FONT}:text='%{pts\\:hms}':x=8:y=8:fontsize=24:fontcolor=yellow:box=1:boxcolor=black@0.6,\
tile=6x5:padding=4:margin=4" -frames:v 1 "$OUT"

echo "wrote $OUT (one frame every ${EVERY}s of $IN)"
