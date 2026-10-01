#!/bin/bash
# Sung-lyric data + continual training run on the training node (~/nv1).
#   start:  setsid nohup ~/nv1/lyric_run.sh > ~/nv1/lyric/run.out 2>&1 < /dev/null &
#   stop:   touch ~/nv1/lyric/STOP   (every part finishes its current item, then exits)
#   status: tail ~/nv1/lyric/run.log; ~/nv1/lyric/eval/*/table.txt
# Parts: one song producer per fleet node (admission respected), the harvester
# (separate, align, filter by line, write shards), continual acoustic training
# in rounds once enough clean sung lyrics exist, and the A/B eval every 2 h.
set -u
cd ~/nv1
L=lyric
B=./makepad/target/release
mkdir -p $L/inbox $L/out prep/lyrics $L/eval runs/lyric
log() { echo "$(date +%FT%T) $*" >> $L/run.log; }
stopped() { test -f $L/STOP; }
NODES=${NODES:-"http://10.0.0.123:8123 http://10.0.0.100:8123"}
MIN_CLEAN=${MIN_CLEAN:-3600}

log "run start, nodes: $NODES"
for n in $NODES; do
  ( i=0; while ! stopped; do
      python3 song_producer.py --node "$n" --inbox $L/inbox --count 5 --seconds 35 --seed $((RANDOM * 32768 + RANDOM + i)) >> $L/producer-$(echo $n | tr -dc '0-9').log 2>&1
      i=$((i + 1))
    done ) &
done

# Harvester: watches the inbox (restarted in rounds so STOP is honoured).
( while ! stopped; do
    timeout 900 $B/sing_data $L/inbox $L/out --shards prep/lyrics --watch --no-wav \
      --whisper models/ggml-large-v3-turbo.bin --stems ~/.makepad/weights/stems/model_bs_roformer_ep_17_sdr_9.6568.ckpt >> $L/harvest.log 2>&1
  done ) &

clean() { awk -F'\t' '{s+=$5} END {printf "%d", s}' prep/lyrics/harvest.tsv 2>/dev/null || echo 0; }

# Trainer: rounds of 3000 steps, each re-indexing the growing data.
( while ! stopped && [ "$(clean)" -lt "$MIN_CLEAN" ]; do sleep 120; done
  stopped && exit 0
  log "training starts with $(clean) s of clean sung lyrics"
  if [ ! -f runs/lyric/ac.mksing ]; then cp runs/a2/ac.mksing runs/a2/ac.opt.mksing runs/a2/ac.ema.mksing runs/lyric/; fi
  r=0
  while ! stopped; do
    $B/sing_train ac --data prep/vocalset --data prep/libritts --data prep/lyrics --config base --resume \
      --steps 3000 --warmup 200 --lr 1e-4 --batch 32 --ac-frames 600 --speech-frames 1000 \
      --mix lyric=0.5,speech=0.25,sung=0.25 --log 500 --save 3000 --workers 12 --out runs/lyric >> runs/lyric/train.log 2>&1 \
      || { log "training round failed (runs/lyric/train.log)"; sleep 300; }
    r=$((r + 1)); log "training round $r done ($(clean) s clean)"
  done ) &

# Eval every 2 h (first one when training has a checkpoint past round 1).
( last=0
  while ! stopped; do
    now=$(date +%s)
    if [ $((now - last)) -ge 7200 ] && grep -q "round 1 done" $L/run.log 2>/dev/null; then
      tag=$(date +%m%d-%H%M)
      $B/sing_train export --ac runs/lyric/ac.ema.mksing --voc runs/v2/voc.ema.mksing --out $L/eval/cantor-$tag.mksing > /dev/null 2>&1
      $B/voice_eval --whisper models/ggml-large-v3-turbo.bin --cantor $L/eval/cantor-$tag.mksing --singer 1600 --out $L/eval/$tag > $L/eval/$tag.txt 2>&1
      grep -A4 "^engine" $L/eval/$tag.txt > $L/eval/$tag/table.txt
      log "eval $tag: $(grep -E '^(dsp|cantor) ' $L/eval/$tag.txt | tr -s ' ' | tr '\n' ';')"
      last=$now
    fi
    sleep 300
  done ) &

wait
log "run stopped"
