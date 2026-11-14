#!/bin/bash
# T-045: cross-process lifecycle from a recorded process group.
set -u
P=$(cd "$(dirname "$0")" && pwd)
B=$P/build
WS=$B/t045-ws
STATE=$B/t045.pid
rm -rf "$B"
mkdir -p "$WS"
cc -O1 -o "$B/launcher" "$P/T-045-launcher.c" || exit 1
cc -O1 -o "$WS/escapee" "$P/T-045-escapee.c" || exit 1

echo "host: $(sw_vers -productVersion) ($(sw_vers -buildVersion)) $(uname -m)"
echo

echo "== launcher exits, group remains =="
"$B/launcher" "$STATE" "$WS" -- /bin/sh -c "i=0; while :; do echo \$i >> '$WS/progress'; i=\$((i+1)); /bin/sleep 0.05; done"
echo "launcher exit=$?"
PGID=$(cat "$STATE"); echo "recorded pgid=$PGID"
sleep 0.4
echo "progress after start: $(wc -c < "$WS/progress") bytes"
ps -o pid,ppid,pgid,sess,command -p "$PGID" | tail -2

echo
echo "== later process: SIGSTOP the group =="
kill -STOP -"$PGID"; echo "kill -STOP exit=$?"
sleep 0.5
A=$(wc -c < "$WS/progress"); sleep 0.3; C=$(wc -c < "$WS/progress")
echo "size a=$A c=$C; unchanged=$([ "$A" = "$C" ] && echo yes || echo no)"

echo
echo "== SIGCONT =="
kill -CONT -"$PGID"; echo "kill -CONT exit=$?"
sleep 0.5
D=$(wc -c < "$WS/progress"); echo "size d=$D; grew=$([ "$D" -gt "$C" ] && echo yes || echo no)"

echo
echo "== SIGKILL the group =="
kill -KILL -"$PGID"; echo "kill -KILL exit=$?"
sleep 0.4
E=$(wc -c < "$WS/progress"); sleep 0.3; F=$(wc -c < "$WS/progress")
echo "size e=$E f=$F; stopped=$([ "$E" = "$F" ] && echo yes || echo no)"
echo "group alive after kill: $(kill -0 -"$PGID" 2>/dev/null && echo yes || echo no)"

echo
echo "== setsid escapee survives the group kill =="
rm -f "$WS/escape" "$WS/escape.pid" /tmp/t045-escape-outside
"$B/launcher" "$STATE" "$WS" -- /bin/sh -c "'$WS/escapee' '$WS/escape' /tmp/t045-escape-outside '$WS/escape.pid' >/dev/null 2>&1 & wait"
PGID=$(cat "$STATE"); echo "recorded pgid=$PGID"
sleep 0.8
ESC=$(cat "$WS/escape.pid")
echo "escapee pid=$ESC"
kill -KILL -"$PGID"; echo "kill -KILL group exit=$?"
sleep 0.5
echo "escapee alive after group kill: $(kill -0 "$ESC" 2>/dev/null && echo yes || echo no)"
echo "escapee ps: $(ps -o pid,ppid,pgid,sess,command -p "$ESC" | tail -1)"
echo "escape file bytes: $(wc -c < "$WS/escape" 2>/dev/null || echo 0)"
echo "out-of-workspace file present: $(test -e /tmp/t045-escape-outside && echo yes || echo no)"
kill -KILL "$ESC" 2>/dev/null
sleep 0.2
echo "escapee after manual kill: $(kill -0 "$ESC" 2>/dev/null && echo alive || echo gone)"
