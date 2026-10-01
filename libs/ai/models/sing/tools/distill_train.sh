#!/bin/bash
# Cantor trained from scratch on the sung-lyric distill (prep/lyrics, growing),
# in rounds proportional to the data: a round runs when the clean set has grown
# by half since the last one; its steps scale with the set; the acoustic model
# holds out 10% of the songs and stops early when they stop improving. After
# each acoustic round, the DSP-vs-neural eval (on the best held-out weights).
#   start: setsid nohup ~/nv1/distill_train.sh > ~/nv1/lyric/distill.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP  (finishes the current round)
set -u
cd ~/nv1
B=./makepad/target/release
L=lyric
log() { echo "$(date +%FT%T) $*" >> $L/run.log; }
stopped() { test -f $L/STOP; }
items() { awk -F'\t' '{s+=$4} END {printf "%d", s}' prep/lyrics/harvest.tsv; }
secs() { awk -F'\t' '{s+=$5} END {printf "%d", s}' prep/lyrics/harvest.tsv; }
clamp() { v=$1; [ $v -lt $2 ] && v=$2; [ $v -gt $3 ] && v=$3; echo $v; }
mkdir -p runs/distill-voc runs/distill-ac $L/eval
GROW=${GROW:-150}   # percent of the last round's data that triggers the next
log "distill training (data-proportional rounds) start: $(items) segments, $(secs) s"

( last=0
  while ! stopped; do
    s=$(secs)
    if [ $((s * 100)) -ge $((last * GROW)) ] || [ $last -eq 0 ]; then
      steps=$(clamp $((20 * s / 64)) 500 30000)   # about 20 passes over the clean audio
      resume=""; [ -f runs/distill-voc/voc.mksing ] && resume="--resume"
      log "vocoder round: $s s clean, $steps steps"
      $B/sing_train voc --data prep/lyrics --config base $resume --steps $steps --warmup 300 --batch 32 --frames 200 \
        --log 500 --save $steps --workers 8 --out runs/distill-voc >> runs/distill-voc/train.log 2>&1 || log "vocoder round failed"
      last=$s
    fi
    sleep 120
  done ) &

( last=0
  while ! stopped; do
    n=$(items)
    if [ $((n * 100)) -ge $((last * GROW)) ] || [ $last -eq 0 ]; then
      steps=$(clamp $((30 * n / 32)) 500 30000)   # at most 30 passes; early stop usually ends it sooner
      resume=""; [ -f runs/distill-ac/ac.mksing ] && resume="--resume"
      log "acoustic round: $n segments, up to $steps steps (10% of songs held out, early stop)"
      rm -f runs/distill-ac/ac-best.*
      $B/sing_train ac --data prep/lyrics --config base $resume --steps $steps --warmup 200 --batch 32 --speech-frames 1000 \
        --mix lyric=1 --holdout 10 --patience 4 --log 250 --save $steps --workers 8 --out runs/distill-ac >> runs/distill-ac/train.log 2>&1 \
        || log "acoustic round failed"
      last=$n
      log "acoustic round done: $(grep 'held-out' runs/distill-ac/train.log | tail -1 | cut -c1-120)"
      # The next round resumes from the best held-out weights, not the overfit end.
      if [ -f runs/distill-ac/ac-best.mksing ]; then
        for x in mksing opt.mksing ema.mksing; do cp runs/distill-ac/ac-best.$x runs/distill-ac/ac.$x; done
      fi
      ac=runs/distill-ac/ac-best.ema.mksing; [ -f $ac ] || ac=runs/distill-ac/ac.ema.mksing
      if [ -f runs/distill-voc/voc.ema.mksing ]; then
        step=$(grep -a -o "^step=[0-9]*" runs/distill-ac/ac-best.mksing 2>/dev/null | head -1 | cut -d= -f2)
        tag="distill-${step:-x}"
        $B/sing_train export --ac $ac --voc runs/distill-voc/voc.ema.mksing --out $L/eval/$tag.mksing > /dev/null 2>&1
        $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $L/eval/$tag.mksing --singer 1600 --out $L/eval/$tag > $L/eval/$tag.txt 2>&1
        log "eval $tag ($n segments): $(grep -E '^(dsp|cantor) ' $L/eval/$tag.txt | tr -s ' ' | tr '\n' ';')"
      fi
    fi
    sleep 120
  done ) &
wait
