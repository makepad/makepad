#!/bin/bash
# Grade-separation eval harness: for each spot in bridge_eval_spots.txt,
# capture a 3D angled render (via the studio remote bridge @cam) and the
# PDOK 7.5cm aerial ortho of the same intersection, into paired images.
#
# Usage: bridge_eval.sh <cmds-file> <bridge-log> <build-id> <out-dir> [spots-file]
# The app must already be running as <build-id> on the given bridge.
set -e
CMDS="$1"; LOG="$2"; BUILD="$3"; OUT="$4"; SPOTS="${5:-$(dirname "$0")/bridge_eval_spots.txt}"
mkdir -p "$OUT"

names=()
before=$(grep -c '"path":"' "$LOG" || true)

while read -r name lon lat zoom rot tilt; do
    [[ "$name" =~ ^#.*$ || -z "$name" ]] && continue
    names+=("$name")
    # Aerial: ~500 m x 330 m window centered on the spot.
    s=$(echo "$lat - 0.0015" | bc -l); n=$(echo "$lat + 0.0015" | bc -l)
    w=$(echo "$lon - 0.0037" | bc -l); e=$(echo "$lon + 0.0037" | bc -l)
    curl -s --max-time 30 "https://service.pdok.nl/hwh/luchtfotorgb/wms/v1_0?service=WMS&request=GetMap&version=1.3.0&layers=Actueel_orthoHR&crs=EPSG:4326&bbox=$s,$w,$n,$e&width=1000&height=660&format=image/jpeg&styles=" \
        -o "$OUT/${name}_aerial.jpg" &
    # Render: drive the app camera and screenshot.
    cat >> "$CMDS" << EOF
{"Click":{"build_id":[$BUILD],"x":192,"y":34}}
#SLEEP 1
{"TypeText":{"build_id":[$BUILD],"text":"@cam $lon $lat $zoom $rot $tilt"}}
#SLEEP 1
{"Return":{"build_id":[$BUILD]}}
#SLEEP 9
{"Screenshot":{"build_id":[$BUILD]}}
#SLEEP 1
EOF
done < "$SPOTS"
wait

# Wait for all screenshots to land in the log, then pair them up in order.
want=$(( before + ${#names[@]} ))
for _ in $(seq 1 240); do
    have=$(grep -c '"path":"' "$LOG" || true)
    [ "$have" -ge "$want" ] && break
    sleep 2
done

paths=$(grep -o '"path":"[^"]*"' "$LOG" | sed 's/"path":"//; s/"//' | tail -n ${#names[@]})
i=0
while IFS= read -r p; do
    cp "$p" "$OUT/${names[$i]}_render.png"
    i=$((i+1))
done <<< "$paths"
echo "eval pairs in $OUT:"
ls "$OUT" | sort
