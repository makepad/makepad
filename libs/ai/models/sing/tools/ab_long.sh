#!/bin/bash
# Long A/B arms: held-out mel loss bottoms out after 1-2k steps from scratch,
# long before the voice is intelligible, so these arms train a fixed budget
# with no early stop, keep the EMA weights every SAVE steps, and score each
# kept checkpoint by held-out word error rate (voice_eval --diag --held).
#   start: setsid nohup ~/nv1/ab_long.sh > ~/nv1/ab/long.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP  (after the current arm)
set -u
cd ~/nv1
B=./makepad/target/release
STEPS=${STEPS:-20000}
SAVE=${SAVE:-4000}
log() { echo "$(date +%FT%T) $*" | tee -a ab/results.txt >> lyric/run.log; }
log "long A/B arms: $STEPS steps each, held-out WER every $SAVE steps, on prep/lyrics-ab"
# name config data mix augment
ARMS=${ARMS:-"base-aug-long:base:lyric:lyric=1:0.5
speech-long:base:speech:lyric=0.75,speech=0.25:0.5"}
for arm in $ARMS; do
  [ -f lyric/STOP ] && break
  IFS=: read name cfg data mix aug <<< "$arm"
  d="--data prep/lyrics-ab"; [ $data = speech ] && d="$d --data prep/libritts"
  out=runs/ab-$name; rm -rf $out; mkdir -p $out
  $B/sing_train ac $d --config $cfg --steps $STEPS --warmup 500 --batch 32 --speech-frames 1000 --mix $mix --augment $aug \
    --holdout 10 --patience 1000000 --log 500 --save $SAVE --keep --workers 8 --out $out > $out/train.log 2>&1 || { log "$name: training failed"; continue; }
  for ck in $out/ac-*.ema.mksing; do
    step=$(basename $ck .ema.mksing | cut -d- -f2 | sed 's/^0*//')
    $B/sing_train export --ac $ck --voc runs/distill-voc/voc.ema.mksing --out $out/cantor-$step.mksing > /dev/null 2>&1
    $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $out/cantor-$step.mksing --diag prep/lyrics-ab --held --items 24 --out $out/held-$step > $out/held-$step.txt 2>/dev/null
    mel=$(grep "step $step | held-out" $out/train.log | awk '{print $6}')
    log "$name step $step: held-out mel $mel; held-out WER $(grep -E "^(tf|free) " $out/held-$step.txt | awk '{printf "%s %s ", $1, $2}')"
  done
done
log "long A/B arms done"
