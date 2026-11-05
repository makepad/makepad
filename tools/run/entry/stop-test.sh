#!/bin/sh
# Offline regression for makepad.sh: stopping the bootstrap ends a recorded job and what it started,
# also a child that ignores TERM, and a prefetch download; a PID file a killed run left behind is not
# acted on. No downloads or builds. Usage: sh stop-test.sh
set -u
here=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
temp=$(mktemp -d) || exit 2
# makepad.sh up to its first helper after the job functions: only what stops and starts jobs
sed '/^sha256()/,$d' "$here/makepad.sh" > "$temp/prefix.sh"
mkfifo "$temp/pid" "$temp/never"
cat > "$temp/child.sh" <<'CHILD'
trap '' TERM
echo "$$" > "$1"
read line < "$2"
CHILD
cat > "$temp/parent.sh" <<'PARENT'
sh "$1" "$2" "$3" &
wait
PARENT
. "$temp/prefix.sh"
jobs=$temp
# What a killed run left behind: its PID is some other process's by now, and that process lives on.
sh "$temp/child.sh" "$temp/pid" "$temp/never" &
read bystander < "$temp/pid"
echo "$bystander" > "$temp/build.pid"
echo "$bystander" > "$temp/build.waiter.pid"
forget_old_jobs
stop_jobs
case "$(ps -p "$bystander" -o stat=)" in ''|*Z*) hit=$bystander ;; *) hit= ; kill -KILL "$bystander" ;; esac
wait "$bystander" 2>/dev/null
job build sh "$temp/parent.sh" "$temp/child.sh" "$temp/pid" "$temp/never"
read child < "$temp/pid"
parent=$(cat "$temp/build.pid")
( sh "$temp/parent.sh" "$temp/child.sh" "$temp/pid" "$temp/never" ) &
prefetch_pids=$!
read prefetch_child < "$temp/pid"
stop_jobs
result=$?
survivors=
for pid in "$parent" "$child" "$prefetch_pids" "$prefetch_child"; do
    state=$(ps -p "$pid" -o stat=)
    case "$state" in ''|*Z*) ;; *) survivors="$survivors $pid"; kill -KILL "$pid" ;; esac
done
rm -f "$temp/pid" "$temp/never" "$temp/child.sh" "$temp/parent.sh" "$temp/prefix.sh" "$temp/build.pid" "$temp/build.done" "$temp/build.waiter.pid"
rmdir "$temp"
if [ "$result" -ne 0 ] || [ -n "$survivors" ] || [ -n "$hit" ]; then
    echo "FAIL exit=$result survivors:$survivors stale PID stopped:$hit"
    exit 1
fi
echo "PASS the job, the download and their children are gone; the stale PID's process was left alone"
