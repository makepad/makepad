#!/bin/bash
# Song producers: one loop per generator node, each separating its own vocals
# in batches (both models stay resident through a batch). NODES lists
# "gen" or "gen@sep" (a node without the separator borrows one), MODEL the
# music model, TAKES songs per lyric set.
#   start: NODES="http://a:8123 http://b:8123@http://a:8123" setsid nohup ~/nv1/producers.sh > ~/nv1/lyric/producers.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP
cd ~/nv1
NODES=${NODES:-"http://10.0.0.123:8123 http://10.0.0.100:8123"}
MODEL=${MODEL:-minimax-music3-q4}
INBOX=${INBOX:-lyric/inbox}
TAKES=${TAKES:-2}
echo "$(date +%FT%T) producers: $MODEL on $NODES, $TAKES takes, into $INBOX" >> lyric/run.log
for e in $NODES; do
  gen=${e%@*}; sep=${e#*@}
  ( while [ ! -f lyric/STOP ]; do
      python3 song_producer.py --model "$MODEL" --node "$gen" --sep-node "$sep" --inbox "$INBOX" --takes "$TAKES" --batch 4 --count 4 \
        >> lyric/producer-$(echo $gen | tr -dc '0-9').log 2>&1
    done ) &
done
wait
