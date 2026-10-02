#!/bin/bash
# Cantor trained from scratch on the sung-lyric distill (prep/lyrics, growing),
# in rounds proportional to the data: a round runs when the clean set has grown
# by half since the last one; its steps scale with the set (about 100 passes);
# half the lyric segments are speed-perturbed. The acoustic model holds out 10%
# of the songs; its checkpoints are scored by held-out word error rate and the
# best is kept (lyric/eval/best.mksing), sung on the eval lines against DSP.
#   start: setsid nohup ~/nv1/distill_train.sh > ~/nv1/lyric/distill.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP  (finishes the current round)
set -u
# A round that kept no checkpoints (the trainer failed to start) has nothing to score.
shopt -s nullglob
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
      steps=$(clamp $((100 * n / 32)) 2000 30000)   # about 100 passes over the set
      half=$((steps / 2))
      resume=""; [ -f runs/distill-ac/ac.mksing ] && resume="--resume"
      log "acoustic round: $n segments, $steps steps, speed augmentation on half the segments; checkpoints scored by held-out WER"
      rm -f runs/distill-ac/ac-0*.ema.mksing
      $B/sing_train ac --data prep/lyrics --config base $resume --steps $steps --warmup 200 --batch 32 --speech-frames 1000 \
        --mix lyric=1 --augment 0.5 --holdout 10 --patience 1000000 --log 500 --save $half --keep --workers 8 \
        --out runs/distill-ac >> runs/distill-ac/train.log 2>&1 || log "acoustic round failed"
      last=$n
      # Held-out mel loss bottoms out long before the voice is intelligible: each
      # kept checkpoint is scored by word error rate on the held-out songs, and the
      # best so far is lyric/eval/best.mksing (its WER in best.txt).
      if [ -f runs/distill-voc/voc.ema.mksing ]; then
        for ck in runs/distill-ac/ac-0*.ema.mksing; do
          step=$(basename $ck .ema.mksing | cut -d- -f2 | sed 's/^0*//')
          tag=distill-$step
          $B/sing_train export --ac $ck --voc runs/distill-voc/voc.ema.mksing --out $L/eval/$tag.mksing > /dev/null 2>&1
          $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $L/eval/$tag.mksing --diag prep/lyrics --held --items 24 --out $L/eval/$tag-held > $L/eval/$tag-held.txt 2>/dev/null
          w=$(grep "^free " $L/eval/$tag-held.txt | awk '{print $2}' | tr -d %)
          [ -n "$w" ] || { log "eval $tag failed"; continue; }
          log "eval $tag ($n segments): held-out WER $(grep -E '^(tf|free) ' $L/eval/$tag-held.txt | awk '{printf "%s %s ", $1, $2}')"
          b=$(cut -d' ' -f1 $L/eval/best.txt 2>/dev/null || echo 101)
          if awk "BEGIN {exit !($w < $b)}"; then
            cp $L/eval/$tag.mksing $L/eval/best.mksing
            echo "$w $tag" > $L/eval/best.txt
            $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $L/eval/best.mksing --singer 1600 --out $L/eval/best-lines > $L/eval/best-lines.txt 2>&1
            log "new best $tag: held-out free WER $w%; eval lines $(grep -E '^(dsp|cantor) ' $L/eval/best-lines.txt | tr -s ' ' | tr '\n' ';')"
          fi
        done
      fi
    fi
    sleep 120
  done ) &
wait
