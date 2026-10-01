#!/bin/bash
# Cantor trained from scratch on the sung-lyric distill (prep/lyrics, growing):
# the vocoder on the separated vocals, the acoustic model on the lyric
# segments; rounds re-index the data; the DSP-vs-neural eval every 2 h.
#   start: setsid nohup ~/nv1/distill_train.sh > ~/nv1/lyric/distill.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP  (finishes the current round)
set -u
cd ~/nv1
B=./makepad/target/release
L=lyric
log() { echo "$(date +%FT%T) $*" >> $L/run.log; }
stopped() { test -f $L/STOP; }
mkdir -p runs/distill-voc runs/distill-ac $L/eval
log "distill training start ($(awk -F'\t' '{s+=$5} END {printf "%d", s}' prep/lyrics/harvest.tsv) s clean)"

( r=0
  while ! stopped; do
    resume=""; [ -f runs/distill-voc/voc.mksing ] && resume="--resume"
    $B/sing_train voc --data prep/lyrics --config base $resume --steps 4000 --warmup 300 --batch 32 --frames 200 \
      --log 500 --save 4000 --workers 8 --out runs/distill-voc >> runs/distill-voc/train.log 2>&1 \
      || { log "vocoder round failed"; sleep 300; }
    r=$((r + 1)); log "vocoder round $r done"
  done ) &

( r=0
  while ! stopped; do
    resume=""; [ -f runs/distill-ac/ac.mksing ] && resume="--resume"
    $B/sing_train ac --data prep/lyrics --config base $resume --steps 2000 --warmup 200 --batch 32 --speech-frames 1000 \
      --mix lyric=1 --log 500 --save 2000 --workers 8 --out runs/distill-ac >> runs/distill-ac/train.log 2>&1 \
      || { log "acoustic round failed"; sleep 300; }
    r=$((r + 1)); log "acoustic round $r done"
  done ) &

( last=0
  while ! stopped; do
    if [ $(( $(date +%s) - last )) -ge 7200 ] && [ -f runs/distill-ac/ac.ema.mksing ] && [ -f runs/distill-voc/voc.ema.mksing ] \
       && [ $(grep -c "acoustic round" $L/run.log) -ge 2 ]; then
      step=$(grep -a -o "^step=[0-9]*" runs/distill-ac/ac.mksing 2>/dev/null | head -1 | cut -d= -f2)
      tag="distill-${step:-x}"
      $B/sing_train export --ac runs/distill-ac/ac.ema.mksing --voc runs/distill-voc/voc.ema.mksing --out $L/eval/$tag.mksing > /dev/null 2>&1
      $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $L/eval/$tag.mksing --singer 1600 --out $L/eval/$tag > $L/eval/$tag.txt 2>&1
      log "eval $tag: $(grep -E '^(dsp|cantor) ' $L/eval/$tag.txt | tr -s ' ' | tr '\n' ';')"
      last=$(date +%s)
    fi
    sleep 120
  done ) &
wait
