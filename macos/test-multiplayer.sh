#!/bin/bash
# Smoke-test LAN multiplayer with the engine bundled in Waroxide.app, on one
# Mac: (1) a waiting host is found by mDNS auto-discovery, like in the Join
# menu, then (2) a host and a client start a 2-player game, the client
# joining through the LAN address (not loopback). Small game windows open
# for a short while.
#
# Usage: macos/test-multiplayer.sh <extracted-data-dir> [host-ip]
#
# Adapted from Stratagus' pytests/test_wargus_multiplayer_process.py.
set -uo pipefail

DATA="${1:?usage: $0 <extracted-data-dir> [host-ip]}"
IP="${2:-$(ipconfig getifaddr en0 || ipconfig getifaddr en1)}"
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
ENGINE="$ROOT/target/macos/Waroxide.app/Contents/MacOS/stratagus"
MAP="maps/skirmish/multiplayer/(2)timeless-isle.smp.gz"
PORT=6661
RUN_AFTER_START=10
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wargus-mp.XXXXXX")"

[ -x "$ENGINE" ] || { echo "build the app first (macos/build-app.sh)"; exit 1; }
[ -n "$IP" ] || { echo "no LAN address found; pass one as the second argument"; exit 1; }
[ -f "$DATA/$MAP" ] || { echo "$DATA does not look like extracted Wargus data (missing $MAP)"; exit 1; }

for who in host client; do
    mkdir -p "$WORK/$who/wc2"
    cat > "$WORK/$who/wc2/preferences.lua" <<'EOF'
wc2 = wc2 or {}
wc2.preferences = wc2.preferences or {}
wc2.preferences.EnableMouseScrolling = false
wc2.preferences.PauseOnLeave = false
wc2.preferences.ShowTips = false
EOF
done

common=(-d "$DATA" -W 640x480 -v 640x480 -g -c scripts/multiplayer.lua)
status=0

# 1. Auto-discovery: a host waiting for more players answers mDNS queries.
mkdir -p "$WORK/probe"
cat > "$WORK/probe.lua" <<'LUA'
SetGameName("wc2")
local function pause(s) local t = os.clock() + s; while os.clock() < t do end end
for i = 1, 8 do
  for _, ip in ipairs(NetworkDiscoverServers(true)) do print("DISCOVERED " .. ip) end
  pause(0.5) -- replies are read by the next query, as in the Join menu
end
os.exit(0)
LUA
"$ENGINE" "${common[@]}" -u "$WORK/host" -P "$PORT" \
    -G "server,race=human,map=$MAP,numplayers=3,player=mac-host" > "$WORK/discovery-host.log" 2>&1 &
WAITING=$!
sleep 4
"$ENGINE" -d "$DATA" -W 320x240 -g -u "$WORK/probe" -P 0 -c "$WORK/probe.lua" > "$WORK/probe.log" 2>&1
kill $WAITING 2>/dev/null; wait $WAITING 2>/dev/null
if grep -q "DISCOVERED $IP" "$WORK/probe.log"; then
    echo "Discovery: found the host at $IP"
else
    echo "Discovery: host NOT found (got: $(grep DISCOVERED "$WORK/probe.log" | tr '\n' ' '))"
    status=1
fi

# 2. A 2-player game: the client joins by IP and both stay in sync.
"$ENGINE" "${common[@]}" -u "$WORK/host" -P "$PORT" \
    -G "server,race=human,map=$MAP,numplayers=2,resources=High,units=1,fow=1,reveal=0,player=mac-host" \
    > "$WORK/host.log" 2>&1 &
HOST=$!
sleep 3
"$ENGINE" "${common[@]}" -u "$WORK/client" -P 0 \
    -G "client,race=orc,ip=$IP,port=$PORT,player=mac-client" \
    > "$WORK/client.log" 2>&1 &
CLIENT=$!
trap 'kill $HOST $CLIENT 2>/dev/null' EXIT

echo "Host on $IP:$PORT, client joining; logs in $WORK"
started=0
for _ in $(seq 70); do
    if grep -q "FINAL NETWORK GAME SETUP" "$WORK/host.log"; then
        echo "Game set up; letting it run ${RUN_AFTER_START}s"
        sleep "$RUN_AFTER_START"
        started=1
        break
    fi
    kill -0 $HOST 2>/dev/null && kill -0 $CLIENT 2>/dev/null || break
    sleep 1
done

[ $started -eq 1 ] || { echo "the game was not set up"; status=1; }
for who in host client; do
    pid=$([ $who = host ] && echo $HOST || echo $CLIENT)
    kill -0 "$pid" 2>/dev/null || { echo "$who exited early"; status=1; }
done
if grep -E "Network out of sync|sent bad command|Unknown unitType|Segmentation fault|Aborted" "$WORK"/*.log; then
    status=1
fi
grep -q "mac-host" "$WORK/host.log" && grep -q "mac-client" "$WORK/host.log" \
    || { echo "the host does not list both players"; status=1; }

if [ $status -eq 0 ]; then
    echo "PASS: discovery works, and host and client started a network game and stayed in sync"
else
    echo "FAIL; last lines of the logs:"
    tail -n 15 "$WORK/host.log" "$WORK/client.log"
fi
exit $status
