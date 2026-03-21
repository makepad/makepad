#!/bin/bash
# Music Player — interactive driver for Canvas audio
# Usage: bash music-player.sh
# Commands: 1/2/3 = play song, p = pause, s = stop, q = quit

PORT=$(cat /tmp/makepad-canvas.port 2>/dev/null || true)
[ -z "$PORT" ] && { echo "Canvas not running."; exit 1; }
API="http://127.0.0.1:$PORT"

SONGS=(
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3"
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-2.mp3"
  "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-3.mp3"
)
NAMES=("Ambient Flow" "Electronic Pulse" "Synth Dream")

# Load splash UI
curl -s -X POST "$API/splash" --data-binary @tools/canvas/examples/music-player.splash > /dev/null

echo "Music Player — Canvas Audio"
echo "  1 = Ambient Flow"
echo "  2 = Electronic Pulse"
echo "  3 = Synth Dream"
echo "  p = pause/resume"
echo "  s = stop"
echo "  q = quit"
echo ""

while true; do
  printf "> "
  read -r cmd
  case "$cmd" in
    1) echo "Playing: ${NAMES[0]}"
       curl -s -X POST "$API/audio/play" -d "${SONGS[0]}" > /dev/null ;;
    2) echo "Playing: ${NAMES[1]}"
       curl -s -X POST "$API/audio/play" -d "${SONGS[1]}" > /dev/null ;;
    3) echo "Playing: ${NAMES[2]}"
       curl -s -X POST "$API/audio/play" -d "${SONGS[2]}" > /dev/null ;;
    p) curl -s -X POST "$API/audio/pause" > /dev/null
       echo "Toggled pause" ;;
    s) curl -s -X POST "$API/audio/stop" > /dev/null
       echo "Stopped" ;;
    q) echo "Bye"; exit 0 ;;
    *) echo "Unknown: $cmd" ;;
  esac
done
