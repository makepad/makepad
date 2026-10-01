#!/bin/bash
# Song producers: one loop per generator node, each song's vocal separated on
# the processing node. GEN and SEP from the environment.
#   start: GEN="http://a:8123 http://b:8123" SEP=http://c:8123 setsid nohup ~/nv1/producers.sh > ~/nv1/lyric/producers.out 2>&1 < /dev/null &
#   stop:  touch ~/nv1/lyric/STOP
cd ~/nv1
GEN=${GEN:-"http://10.0.0.123:8123 http://10.0.0.100:8123"}
SEP=${SEP:-http://10.0.0.166:8123}
echo "$(date +%FT%T) producers: gen $GEN, separation $SEP" >> lyric/run.log
for n in $GEN; do
  ( i=0; while [ ! -f lyric/STOP ]; do
      python3 song_producer.py --node "$n" --sep-node "$SEP" --inbox lyric/inbox --count 5 --seconds 35 --seed $((RANDOM * 32768 + RANDOM + i)) >> lyric/producer-$(echo $n | tr -dc '0-9').log 2>&1
      i=$((i + 1))
    done ) &
done
wait
