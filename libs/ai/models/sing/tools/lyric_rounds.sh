#!/bin/bash
# Data-growth rounds for the sung-lyric voice (node 165). When the harvested
# clean set has grown by half since the last round: pause the GPU's other
# users (the AI hub and the harvester; the song producers only use the
# generator nodes and keep running), rebuild the score-aligned set, train the
# acoustic model from the speech-trained A1 weights, pick the round's best
# checkpoint by held-out WER, and keep it only if it beats the current best on
# the eval lines on both word error and note pitch. Then restart what was
# paused, exactly as it ran, and check it is back.
#   start: setsid nohup ~/sr1/lyric_rounds.sh > ~/sr1/rounds/out.log 2>&1 < /dev/null &
#   stop:  touch ~/sr1/rounds/STOP   (between rounds; a running round finishes)
# Log: ~/sr1/rounds/run.log
set -u
shopt -s nullglob
ulimit -n 16384
S=~/sr1; R=$S/rounds; B=$S/makepad/target/release; N=~/nv1
W=$N/models/ggml-large-v3-turbo.bin
VOC=$N/runs/distill-voc/voc.ema.mksing
BEST=$N/lyric/eval/best.mksing
GROW=${GROW:-150}          # percent of the last round's clean seconds that starts a round
mkdir -p $R
log() { echo "$(date +%FT%T) $*" >> $R/run.log; }
secs() { awk -F'\t' '{s+=$5} END {printf "%d", s}' $N/prep/lyrics/harvest.tsv; }
last=$(cat $R/last-secs 2>/dev/null || secs)
echo $last > $R/last-secs
log "rounds start: $(secs) s harvested, next round at $((last * GROW / 100)) s"

# --- pause / restart ---------------------------------------------------------
HARVEST_CMD='cd ~/nv1; while [ ! -f lyric/STOP ]; do timeout 900 ./makepad/target/release/sing_data lyric/inbox lyric/out --shards prep/lyrics --watch --no-wav --whisper models/ggml-large-v3-turbo.bin --stems /home/arch/.makepad/weights/stems/model_bs_roformer_ep_17_sdr_9.6568.ckpt >> lyric/harvest.log 2>&1; done'
paused_hub=0; paused_harvest=0
pause() {
  local p
  ps -eo pid=,args= > $R/ps-before-pause.txt
  # The harvester loop: the shell running exactly HARVEST_CMD, its timeout and sing_data.
  for p in $(ps -eo pid= ); do
    [ "$(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null)" = "bash -c $HARVEST_CMD " ] || continue
    local kids=$(ps -o pid= --ppid $p); local gkids=""
    for k in $kids; do gkids="$gkids $(ps -o pid= --ppid $k)"; done
    log "pause: harvester loop $p (children $kids $gkids)"
    kill $p; kill $kids $gkids 2>/dev/null
    paused_harvest=1
  done
  if systemctl --user is-active -q makepad-aihub-live.service; then
    log "pause: AI hub (makepad-aihub-live, pid $(systemctl --user show -p MainPID --value makepad-aihub-live.service))"
    systemctl --user stop makepad-aihub-live.service && paused_hub=1
  fi
}
restart() {
  if [ $paused_harvest = 1 ]; then
    ( export CUDAHOSTCXX=/usr/bin/g++-15 CUDA_PATH=/opt/cuda NVCC_CCBIN=/usr/bin/g++-15 CUDA_HOME=/opt/cuda
      cd ~/nv1 && setsid nohup bash -c "$HARVEST_CMD" >> lyric/run.out 2>&1 < /dev/null & )
    paused_harvest=0
  fi
  if [ $paused_hub = 1 ]; then
    systemctl --user reset-failed makepad-aihub-live.service 2>/dev/null
    systemd-run --user --unit=makepad-aihub-live /usr/local/bin/makepad-aihub-session > /dev/null 2>&1
    paused_hub=0
  fi
  sleep 20
  local hub=$(systemctl --user is-active makepad-aihub-live.service) port=$(ss -ltn | grep -c ':8785 ')
  local hv=$(ps -eo args= | grep -c '^timeout 900 ./makepad/target/release/sing_data')
  log "restarted: hub $hub (port 8785 listening: $port), harvester sing_data running: $hv"
}
trap 'restart; log "rounds stopped"; exit 0' TERM INT

# --- one round -----------------------------------------------------------------
# eval_lines <mksing> <tag>: mean WER and note pitch (without octave-read notes) over 3 melody sets.
eval_lines() {
  local k
  for k in 0 1 2; do
    nice -n 10 $B/voice_eval --whisper $W --cantor $1 --singer 1600 --melodies $k --out $D/lines-$2-m$k > $D/lines-$2-m$k.txt 2>&1 &
  done
  wait
  cat $D/lines-$2-m*.txt | awk '$1 == "cantor" { w += $2; p += $10; n++ } END { if (n == 3) printf "%.1f %.2f", w / 3, p / 3; else print "x x" }'
}
round() {
  local r=$1 hours steps s best bw
  D=$R/r$r; mkdir -p $D
  log "round $r: $(secs) s harvested"
  pause
  rm -rf $D/words $D/sa
  nice -n 10 python3 $S/words_extract.py $D/words >> $D/prep.log 2>&1
  nice -n 10 $B/sing_prep lyrics $D/words $D/sa >> $D/prep.log 2>&1 || { log "round $r: prep failed"; restart; return 1; }
  hours=$(tail -1 $D/prep.log | awk '{print $5}')
  # About 6000 steps at 6.7 h (best held-out WER at 5-6k), scaled with the data.
  steps=$(awk -v h=$hours 'BEGIN { s = int(6000 * h / 6.66 / 1000 + 0.5) * 1000; if (s < 6000) s = 6000; if (s > 16000) s = 16000; print s }')
  log "round $r: $(tail -1 $D/prep.log); acoustic $steps steps from A1"
  nice -n 5 $B/sing_train ac --data $D/sa --config base --init $N/runs/a1/ac.ema.mksing --steps $steps --warmup 500 --batch 32 \
    --speech-frames 1000 --mix lyric=1 --augment 0.5 --dropout 0.2 --holdout 10 --patience 1000000 --log 500 --save 1000 --keep \
    --workers 8 --out $D/ac > $D/ac.log 2>&1 || { log "round $r: acoustic failed ($D/ac.log)"; restart; return 1; }
  best=""; bw=1000
  for s in $(seq 2000 1000 $steps); do
    $B/sing_train export --ac $D/ac/ac-$(printf %07d $s).ema.mksing --voc $VOC --f0-trained --out $D/ac-$s.mksing > /dev/null 2>&1
    nice -n 10 $B/sing_eval --whisper $W --cantor $D/ac-$s.mksing --words $D/words --items 40 --wavs 0 --out $D/held-$s > $D/held-$s.txt 2>&1 &
    [ $(jobs -r | wc -l) -ge 3 ] && wait -n
  done
  wait
  for s in $(seq 2000 1000 $steps); do
    local fr=$(awk '$1 == "free" { gsub("%", "", $2); print $2 }' $D/held-$s.txt)
    log "round $r: step $s held-out WER tf $(awk '$1 == "tf" {print $2}' $D/held-$s.txt) free $fr%"
    [ -n "$fr" ] && awk "BEGIN {exit !($fr < $bw)}" && { bw=$fr; best=$s; }
  done
  [ -n "$best" ] || { log "round $r: no scored checkpoint"; restart; return 1; }
  # The incumbent on this round's held-out songs, for the log.
  nice -n 10 $B/sing_eval --whisper $W --cantor $BEST --words $D/words --items 40 --wavs 0 --out $D/held-incumbent > $D/held-incumbent.txt 2>&1
  local inc=$(awk '$1 == "free" {print $2}' $D/held-incumbent.txt)
  read cw cand_p <<< "$(eval_lines $D/ac-$best.mksing cand)"
  read iw inc_p <<< "$(eval_lines $BEST best)"
  log "round $r: candidate step $best (held-out free $bw%, incumbent $inc): eval lines WER $cw% pitch $cand_p c; incumbent $iw% $inc_p c"
  if awk "BEGIN {exit !($cw < $iw && $cand_p <= $inc_p)}"; then
    cp $BEST $N/lyric/eval/best-before-r$r-$(date +%m%d-%H%M).mksing
    cp $D/ac-$best.mksing $BEST
    echo "$bw sr1-round$r-ac$best (held-out free; eval lines WER $cw% pitch $cand_p c, 3 melody sets)" > $N/lyric/eval/best.txt
    log "round $r: NEW BEST installed ($BEST)"
  else
    log "round $r: kept the incumbent"
  fi
  restart
}

r=$(ls -d $R/r* 2>/dev/null | wc -l)
while [ ! -f $R/STOP ]; do
  now=$(secs)
  if [ $((now * 100)) -ge $((last * GROW)) ]; then
    r=$((r + 1))
    round $r
    last=$now; echo $last > $R/last-secs
    log "next round at $((last * GROW / 100)) s"
  fi
  sleep 600
done
log "rounds stopped (STOP)"
