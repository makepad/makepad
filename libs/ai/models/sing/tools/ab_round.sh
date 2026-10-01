#!/bin/bash
# A/B round against memorisation: acoustic arms trained from scratch on one
# frozen snapshot of the sung-lyric set (10% of songs held out by name),
# each until its held-out loss stops improving, then scored on the held-out
# songs (voice_eval --diag --held) and on the eval lines (voice_eval).
#   start: setsid nohup ~/nv1/ab_round.sh > ~/nv1/ab/run.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP  (after the current arm)
set -u
cd ~/nv1
B=./makepad/target/release
AB=ab
STEPS=${STEPS:-12000}
mkdir -p $AB prep/lyrics-ab
rm -f prep/lyrics-ab/*.mksdat
cp -l prep/lyrics/*.mksdat prep/lyrics-ab/
log() { echo "$(date +%FT%T) $*" | tee -a $AB/results.txt >> lyric/run.log; }
log "A/B round: $(ls prep/lyrics-ab | wc -l) lyric shards frozen in prep/lyrics-ab, up to $STEPS steps per arm"
# name config data mix augment
ARMS="base:base:lyric:lyric=1:0
base-aug:base:lyric:lyric=1:0.5
A-speech:base:speech:lyric=0.75,speech=0.25:0.5
B-small:small:lyric:lyric=1:0.5
AB-small-speech:small:speech:lyric=0.75,speech=0.25:0.5"
for arm in $ARMS; do
  [ -f lyric/STOP ] && break
  IFS=: read name cfg data mix aug <<< "$arm"
  d="--data prep/lyrics-ab"; [ $data = speech ] && d="$d --data prep/libritts"
  out=runs/ab-$name; rm -rf $out; mkdir -p $out
  $B/sing_train ac $d --config $cfg --steps $STEPS --warmup 500 --batch 32 --speech-frames 1000 --mix $mix --augment $aug \
    --holdout 10 --patience 8 --log 250 --save $STEPS --workers 8 --out $out > $out/train.log 2>&1 || { log "$name: training failed"; continue; }
  $B/sing_train export --ac $out/ac-best.ema.mksing --voc runs/distill-voc/voc.ema.mksing --out $out/cantor.mksing > /dev/null 2>&1
  $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $out/cantor.mksing --diag prep/lyrics-ab --held --items 24 --out $out/held > $out/held.txt 2>/dev/null
  $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $out/cantor.mksing --singer 1600 --out $out/eval > $out/eval.txt 2>/dev/null
  best=$(grep "held-out" $out/train.log | awk '{print $6}' | sort -n | head -n 1)
  step=$(grep -a -o "^step=[0-9]*" $out/ac-best.mksing | head -n 1 | cut -d= -f2)
  held=$(grep -E "^(tf|free) " $out/held.txt | awk '{printf "%s %s ", $1, $2}')
  ev=$(grep -E "^cantor " $out/eval.txt | awk '{print $2}')
  log "$name: best held-out mel $best at step $step; held-out WER $held; eval lines WER $ev"
done
log "A/B round done"
