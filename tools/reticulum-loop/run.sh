#!/usr/bin/env bash
# Real Reticulum on two real coppads over the pat-loop ALSA file cable.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
COPPAD="$REPO/target/release/coppad"
ALLOW_DEBUG=0
KEEP=0
TIMEOUT=300
while [[ $# -gt 0 ]]; do
    case "$1" in
        --coppad) COPPAD=${2:?}; shift 2 ;;
        --allow-debug) ALLOW_DEBUG=1; shift ;;
        --keep) KEEP=1; shift ;;
        --timeout) TIMEOUT=${2:?}; shift 2 ;;
        --help) echo 'Usage: run.sh [--coppad PATH] [--allow-debug] [--keep] [--timeout SECONDS]'; exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done
[[ -x "$COPPAD" ]] || { echo 'Build release coppad with --features cpal-backend,kiss-tnc' >&2; exit 2; }
COPPAD=$(realpath "$COPPAD")
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || exit 2
if [[ "$COPPAD" == */target/debug/* && $ALLOW_DEBUG -eq 0 ]]; then
    echo 'Refusing debug coppad: use release or --allow-debug' >&2; exit 2
fi
SCRATCH=$(mktemp -d /tmp/reticulum-loop.XXXXXX)
PIDS=()
cleanup() {
    local pid
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    for pid in "${PIDS[@]}"; do wait "$pid" 2>/dev/null || true; done
    if [[ $KEEP -eq 0 ]]; then rm -rf "$SCRATCH"; fi
}
trap cleanup EXIT
python3 -m venv "$SCRATCH/venv"
"$SCRATCH/venv/bin/pip" -q install rns==1.5.7
# Ask the OS for six distinct ports while holding them all to avoid duplicates.
read -r A_CMD A_DATA A_KISS B_CMD B_DATA B_KISS < <(python3 - <<'EOF'
import socket
sockets = [socket.socket() for _ in range(6)]
for s in sockets:
    s.bind(("127.0.0.1", 0))
print(*(s.getsockname()[1] for s in sockets))
EOF
)
echo "reticulum-loop: RNS 1.5.7, $("$COPPAD" --version)"
echo "reticulum-loop: scratch $SCRATCH"
setup_station() {
    local name=$1 call=$2 cmd=$3 data=$4 kiss=$5 device=$6
    local dir="$SCRATCH/$name"
    mkdir -p "$dir/home" "$dir/rns"
    sed "s|@DIR@|$SCRATCH|g" "$REPO/tools/pat-loop/asoundrc.in" >"$dir/home/.asoundrc"
    cat >"$dir/coppad.toml" <<EOF
[audio]
# Keep live input while synchronous encode/decode runs on the event loop.
buffer_size = 131072
sample_rate = 48000
input_device = "$device"
output_device = "$device"
[radio]
ptt_method = "none"
[host]
bind_address = "127.0.0.1"
vara_enabled = true
vara_command_port = $cmd
vara_data_port = $data
kiss_enabled = true
kiss_port = $kiss
[engine]
profile = "HF_STANDARD"
callsign = "$call"
EOF
    cat >"$dir/rns/config" <<EOF
[reticulum]
  enable_transport = False
  share_instance = No
[interfaces]
  [[Coppa HF]]
    type = TCPClientInterface
    interface_enabled = True
    kiss_framing = True
    target_host = 127.0.0.1
    target_port = $kiss
    fixed_mtu = 500
    bitrate = 600
EOF
}
setup_station a N1CALL "$A_CMD" "$A_DATA" "$A_KISS" coppa-loop-A
setup_station b N0CALL "$B_CMD" "$B_DATA" "$B_KISS" coppa-loop-B
python3 "$REPO/tools/pat-loop/relay.py" "$SCRATCH" "$A_CMD" "$B_CMD" >"$SCRATCH/relay.log" 2>&1 &
PIDS+=($!)
for fifo in ab_in ab_out ba_in ba_out; do
    deadline=$((SECONDS + 10))
    until [[ -p "$SCRATCH/$fifo" ]]; do
        ((SECONDS < deadline)) || { KEEP=1; echo "FAIL: relay startup ($SCRATCH)"; exit 1; }
        sleep 0.1
    done
done
for name in a b; do
    env HOME="$SCRATCH/$name/home" "$COPPAD" --config "$SCRATCH/$name/coppad.toml" >"$SCRATCH/$name/coppad.log" 2>&1 &
    PIDS+=($!)
done
for port in "$A_KISS" "$B_KISS"; do
    deadline=$((SECONDS + 60))
    until (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
        ((SECONDS < deadline)) || { KEEP=1; echo "FAIL: coppad startup ($SCRATCH)"; exit 1; }
        sleep 0.2
    done
done
"$SCRATCH/venv/bin/python" "$HERE/node.py" "$SCRATCH/a/rns" a b >"$SCRATCH/a/node.log" 2>&1 &
PIDS+=($!)
"$SCRATCH/venv/bin/python" "$HERE/node.py" "$SCRATCH/b/rns" b a >"$SCRATCH/b/node.log" 2>&1 &
PIDS+=($!)
deadline=$((SECONDS + TIMEOUT))
while ((SECONDS < deadline)); do
    if grep -q 'RECEIVED DATAGRAM 300 bytes ok=True' "$SCRATCH/a/node.log" &&
       grep -q 'RECEIVED DATAGRAM 300 bytes ok=True' "$SCRATCH/b/node.log"; then break; fi
    sleep 1
done
FAILED=0
for name in a b; do
    if grep -q 'RECEIVED DATAGRAM 300 bytes ok=True' "$SCRATCH/$name/node.log"; then
        echo "peer->$name RECEIVED DATAGRAM 300 bytes ok=True"
    else
        echo "peer->$name no matching datagram"; FAILED=1
    fi
done
if grep -q 'Audio input buffer overflow' "$SCRATCH/a/coppad.log" "$SCRATCH/b/coppad.log"; then
    echo 'Audio input buffer overflow'; FAILED=1
fi
if [[ $FAILED -eq 0 ]]; then echo PASS; else KEEP=1; echo "FAIL: logs kept at $SCRATCH"; fi
[[ $FAILED -eq 0 ]]
