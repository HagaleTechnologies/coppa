#!/usr/bin/env bash
# tools/pat-loop/run.sh: real Pat against two real coppad processes over a
# userspace ALSA audio loop (COP-46). Linux only; no root, no sound card.
#
# Station A (N1CALL) and station B (N0CALL) are two release `coppad`
# processes, each on its own ALSA `file` PCM (asoundrc.in), joined by
# relay.py, a paced half-duplex cable. Pat dials one coppad with
# `pat connect varahf:///CALL` while a second Pat listens on the other, and
# the run checks Pat's exit code, the delivered message body, and both
# coppad logs. See README.md next to this file.
set -euo pipefail

PAT_VERSION=1.0.0
PAT_SHA256=331e4700c9e8b44098d7173e448da53204f59720c6c19ebc903685666aa377f7
PAT_URL="https://github.com/la5nta/pat/releases/download/v${PAT_VERSION}/pat_${PAT_VERSION}_linux_amd64.tar.gz"

A_CALL=N1CALL
B_CALL=N0CALL

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)

usage() {
    cat <<'EOF'
Usage: tools/pat-loop/run.sh [options]

Runs real Pat (built-in varahf transport) against two release coppad
processes joined by a userspace ALSA audio loop, and prints PASS or FAIL.

Options:
  --pat PATH          Pat binary for both stations (default: download pinned
                      Pat v1.0.0 into a cache dir and check its sha256)
  --dial-pat PATH     a different Pat for the dialing station (e.g. v0.13.1)
  --legacy-config     write the dialer's Pat config in the Pat 0.13.x form
                      (varahf host/cmdPort/dataPort instead of addr)
  --coppad PATH       coppad binary (default: target/release/coppad)
  --allow-debug       allow a coppad under target/debug (expected to FAIL:
                      a debug build drops input while decoding; docs/PAT.md)
  --direction a|b|both
                      a: N1CALL dials N0CALL; b: N0CALL dials N1CALL
                      (default: both)
  --body-bytes N      message body size in bytes (default: a one-line body)
  --timeout SECS      limit for each `pat connect` (default: 900)
  --keep              keep the scratch directory and its logs
  --help              show this help

Exit status: 0 when every direction passed, 1 when any failed, 2 on a usage
or setup error (including a Pat download whose sha256 does not match).
EOF
}

die() {
    echo "pat-loop: $*" >&2
    exit 2
}

PAT=""
DIAL_PAT=""
LEGACY=0
COPPAD="$REPO/target/release/coppad"
ALLOW_DEBUG=0
DIRECTION=both
BODY_BYTES=0
TIMEOUT=900
KEEP=0
SELF_TEST_MISMATCH=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pat) PAT=${2:?--pat needs a path}; shift 2 ;;
        --dial-pat) DIAL_PAT=${2:?--dial-pat needs a path}; shift 2 ;;
        --legacy-config) LEGACY=1; shift ;;
        --coppad) COPPAD=${2:?--coppad needs a path}; shift 2 ;;
        --allow-debug) ALLOW_DEBUG=1; shift ;;
        --direction) DIRECTION=${2:?--direction needs a|b|both}; shift 2 ;;
        --body-bytes) BODY_BYTES=${2:?--body-bytes needs a number}; shift 2 ;;
        --timeout) TIMEOUT=${2:?--timeout needs seconds}; shift 2 ;;
        --keep) KEEP=1; shift ;;
        # Hidden: corrupt the expected body, to prove the body check bites.
        --self-test-mismatch) SELF_TEST_MISMATCH=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done

case "$DIRECTION" in a | b | both) ;; *) die "--direction must be a, b or both" ;; esac
[[ "$BODY_BYTES" =~ ^[0-9]+$ ]] || die "--body-bytes must be a number"
[[ "$TIMEOUT" =~ ^[0-9]+$ ]] || die "--timeout must be a number of seconds"
[[ "$(uname -s)" == Linux ]] || die "Linux only (ALSA file PCMs)"
command -v python3 >/dev/null || die "python3 is required"

[[ -x "$COPPAD" ]] || die "no coppad at $COPPAD; build it with:
  cargo build --release -p coppa-daemon --bin coppad --features cpal-backend"
COPPAD=$(cd "$(dirname "$COPPAD")" && pwd)/$(basename "$COPPAD")
if [[ "$COPPAD" == */target/debug/* && $ALLOW_DEBUG -eq 0 ]]; then
    die "refusing a debug coppad ($COPPAD): a debug build cannot decode in real
time, drops input ('Audio input buffer overflow') and misses the connect
request (docs/PAT.md, Build). Use a release build, or pass --allow-debug."
fi

# Download the pinned Pat once into a cache, and check the cached tarball's
# sha256 on every run before extracting it, refusing a mismatch.
fetch_pat() {
    local cache="${XDG_CACHE_HOME:-$HOME/.cache}/coppa-pat-loop/pat-$PAT_VERSION"
    local tarball="$cache/pat_${PAT_VERSION}_linux_amd64.tar.gz"
    mkdir -p "$cache"
    if [[ ! -s "$tarball" ]]; then
        command -v curl >/dev/null || die "curl is required to download Pat"
        curl -fsSL -o "$tarball" "$PAT_URL" || die "could not download $PAT_URL"
    fi
    local got
    got=$(sha256sum "$tarball" | cut -d' ' -f1)
    if [[ "$got" != "$PAT_SHA256" ]]; then
        rm -f "$tarball"
        die "Pat tarball sha256 mismatch: got $got, expected $PAT_SHA256"
    fi
    tar -xzf "$tarball" -C "$SCRATCH" 2>/dev/null
    echo "$SCRATCH/pat_${PAT_VERSION}_linux_amd64/pat"
}

SCRATCH=$(mktemp -d /tmp/pat-loop.XXXXXX)
PIDS=()

# Send SIGTERM to a process this script started. One that is already gone
# exited on its own, which is worth a line on stderr but must not stop the
# teardown of the others.
stop() {
    kill "$1" 2>/dev/null || echo "pat-loop: process $1 had already exited" >&2
}

# Reap a process `stop` signalled. 0 (a clean shutdown) and 143 (128 +
# SIGTERM) are the expected statuses; any other is reported, not fatal.
reap() {
    local status=0
    wait "$1" 2>/dev/null || status=$?
    if ((status != 0 && status != 143)); then
        echo "pat-loop: process $1 exited with status $status" >&2
    fi
}

# Kill only processes this script started, by PID. Never kill by a
# command-line pattern: the pattern can match this shell's own command line
# and kill the harness.
cleanup() {
    local pid
    for pid in "${PIDS[@]}"; do
        stop "$pid"
    done
    for pid in "${PIDS[@]}"; do
        reap "$pid"
    done
    if [[ $KEEP -eq 0 ]]; then
        rm -rf "$SCRATCH"
    fi
}
trap cleanup EXIT

if [[ -z "$PAT" ]]; then
    PAT=$(fetch_pat)
    PAT_LABEL="Pat v$PAT_VERSION (sha256 OK)"
else
    [[ -x "$PAT" ]] || die "no Pat at $PAT"
    PAT_LABEL="Pat $PAT"
fi
DIAL_PAT=${DIAL_PAT:-$PAT}
[[ -x "$DIAL_PAT" ]] || die "no Pat at $DIAL_PAT"


# A free TCP port whose successor is also free (Pat uses command port + 1
# for data), asked of the OS rather than fixed.
free_port_pair() {
    python3 - <<'EOF'
import socket
while True:
    a = socket.socket()
    a.bind(("127.0.0.1", 0))
    port = a.getsockname()[1]
    b = socket.socket()
    try:
        b.bind(("127.0.0.1", port + 1))
    except (OSError, OverflowError):
        a.close(); b.close(); continue
    a.close(); b.close()
    print(port)
    break
EOF
}

free_port() {
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'
}

wait_for_port() {
    local port=$1 deadline=$((SECONDS + 60))
    while ! (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
        ((SECONDS < deadline)) || return 1
        sleep 0.2
    done
}

wait_for_line() {
    local file=$1 pattern=$2 deadline=$((SECONDS + 60))
    while ! grep -q -- "$pattern" "$file" 2>/dev/null; do
        ((SECONDS < deadline)) || return 1
        sleep 0.2
    done
}

A_CMD=$(free_port_pair)
B_CMD=$(free_port_pair)
while ((B_CMD == A_CMD || B_CMD == A_CMD + 1 || B_CMD + 1 == A_CMD)); do
    B_CMD=$(free_port_pair)
done
# Pat's http listeners must not land on a coppad port picked above.
taken() {
    local p=$1
    ((p == A_CMD || p == A_CMD + 1 || p == B_CMD || p == B_CMD + 1))
}
A_HTTP=$(free_port)
while taken "$A_HTTP"; do A_HTTP=$(free_port); done
B_HTTP=$(free_port)
while taken "$B_HTTP" || ((B_HTTP == A_HTTP)); do B_HTTP=$(free_port); done

echo "pat-loop: $PAT_LABEL, coppad $COPPAD"
echo "pat-loop: scratch $SCRATCH, ports A=$A_CMD/$((A_CMD + 1)) B=$B_CMD/$((B_CMD + 1))"

# Per-station isolation: own HOME (for .asoundrc) and XDG dirs.
setup_station() {
    local name=$1 call=$2 cmd=$3 http=$4 device=$5 listen=$6 legacy=$7
    local dir="$SCRATCH/$name"
    mkdir -p "$dir/home" "$dir/xdg/config" "$dir/xdg/state/pat" "$dir/xdg/data" "$dir/mbox"
    sed "s|@DIR@|$SCRATCH|g" "$HERE/asoundrc.in" >"$dir/home/.asoundrc"

    cat >"$dir/coppad.toml" <<EOF
[audio]
sample_rate = 48000
input_device = "$device"
output_device = "$device"

[radio]
ptt_method = "none"

[host]
bind_address = "127.0.0.1"
vara_enabled = true
vara_command_port = $cmd
vara_data_port = $((cmd + 1))

[engine]
profile = "HF_STANDARD"
callsign = "$call"
EOF

    local varahf
    if [[ $legacy -eq 1 ]]; then
        varahf="{ \"host\": \"localhost\", \"cmdPort\": $cmd, \"dataPort\": $((cmd + 1)), \"bandwidth\": 2300 }"
    else
        varahf="{ \"addr\": \"localhost:$cmd\", \"bandwidth\": 2300 }"
    fi
    # version_reporting_disabled and the pre-seeded account-confirmed stamp
    # are TEST-ONLY: they keep Pat from calling the Winlink network (version
    # report, account-activation check) for these made-up callsigns.
    cat >"$dir/pat.json" <<EOF
{
  "mycall": "$call",
  "listen": $listen,
  "http_addr": "localhost:$http",
  "version_reporting_disabled": true,
  "varahf": $varahf
}
EOF
    date -u +'"%Y-%m-%dT%H:%M:%SZ"' >"$dir/xdg/state/pat/.account-confirmed_$call.json"
}

# The environment of station $1 (its own HOME and XDG dirs), in STATION_ENV,
# for `env "${STATION_ENV[@]}" cmd`. A backgrounded simple command's $! is the
# command's own PID; a backgrounded shell function's would be a subshell's,
# and killing that leaves the real process running.
station_env() {
    local dir="$SCRATCH/$1"
    STATION_ENV=(HOME="$dir/home" XDG_CONFIG_HOME="$dir/xdg/config"
        XDG_STATE_HOME="$dir/xdg/state" XDG_DATA_HOME="$dir/xdg/data")
}

setup_station a "$A_CALL" "$A_CMD" "$A_HTTP" coppa-loop-A '[]' 0
setup_station b "$B_CALL" "$B_CMD" "$B_HTTP" coppa-loop-B '["varahf"]' 0

python3 "$HERE/relay.py" "$SCRATCH" "$A_CMD" "$B_CMD" >"$SCRATCH/relay.log" 2>&1 &
PIDS+=($!)
for fifo in ab_in ab_out ba_in ba_out; do
    deadline=$((SECONDS + 10))
    until [[ -p "$SCRATCH/$fifo" ]]; do
        ((SECONDS < deadline)) || die "relay did not create its FIFOs ($SCRATCH/relay.log)"
        sleep 0.1
    done
done

for name in a b; do
    station_env "$name"
    env "${STATION_ENV[@]}" "$COPPAD" --config "$SCRATCH/$name/coppad.toml" \
        >"$SCRATCH/$name/coppad.log" 2>&1 &
    PIDS+=($!)
done
wait_for_port "$A_CMD" || die "coppad A never opened its command port ($SCRATCH/a/coppad.log)"
wait_for_port "$B_CMD" || die "coppad B never opened its command port ($SCRATCH/b/coppad.log)"

# Lines matching $1 across both coppad logs. grep -c prints 0 and exits 1
# when nothing matches, which is a count; exit 2 (a missing or unreadable
# log) is not, so the function fails. Call it as the only substitution in an
# assignment (`x=$(count_both ...)`): that assignment then fails too and
# `set -e` stops the run, where a second substitution in the same
# expression would supply the exit status instead.
count_both() {
    local log n status total=0
    for log in "$SCRATCH/a/coppad.log" "$SCRATCH/b/coppad.log"; do
        status=0
        n=$(grep -c -- "$1" "$log") || status=$?
        ((status <= 1)) || die "cannot count '$1' in $log"
        total=$((total + n))
    done
    echo "$total"
}

make_body() {
    if [[ $BODY_BYTES -eq 0 ]]; then
        echo "Hello $2 from $1, COP-46 pat-loop check."
    else
        python3 - "$BODY_BYTES" <<'EOF'
import random, string, sys
# Seeded random letters: deterministic, and close to incompressible, so the
# message really spans many frames after Pat's gzip.
n = int(sys.argv[1])
rng = random.Random(46)
words = []
while sum(len(w) + 1 for w in words) < n:
    words.append("".join(rng.choice(string.ascii_lowercase) for _ in range(rng.randint(2, 9))))
lines, cur = [], ""
for w in words:
    if len(cur) + len(w) + 1 > 70:
        lines.append(cur)
        cur = w
    else:
        cur = f"{cur} {w}" if cur else w
lines.append(cur)
print("\n".join(lines)[: max(n - 1, 1)])
EOF
    fi
}

FAILED=0
LOGS=()

# One Winlink P2P session: $dialer composes and dials, $listener answers.
run_direction() {
    local dialer=$1 listener=$2
    local d_call l_call
    if [[ $dialer == a ]]; then
        d_call=$A_CALL; l_call=$B_CALL
    else
        d_call=$B_CALL; l_call=$A_CALL
    fi
    local run="$SCRATCH/run-$dialer$listener"
    mkdir -p "$run"
    local subject="pat-loop $d_call to $l_call $$"

    station_env "$listener"
    env "${STATION_ENV[@]}" "$PAT" --config "$SCRATCH/$listener/pat.json" \
        --mbox "$SCRATCH/$listener/mbox" --listen=varahf http >"$run/listener.log" 2>&1 &
    local listener_pid=$!
    PIDS+=("$listener_pid")
    wait_for_line "$run/listener.log" "Listening for incoming traffic" ||
        echo "pat-loop: warning: listener never logged 'Listening' ($run/listener.log)" >&2

    local dial_cfg="$SCRATCH/$dialer/pat.json"
    if [[ $LEGACY -eq 1 ]]; then
        dial_cfg="$run/dialer-legacy.json"
        local cmd
        if [[ $dialer == a ]]; then cmd=$A_CMD; else cmd=$B_CMD; fi
        cat >"$dial_cfg" <<EOF
{
  "mycall": "$d_call",
  "listen": [],
  "version_reporting_disabled": true,
  "varahf": { "host": "localhost", "cmdPort": $cmd, "dataPort": $((cmd + 1)), "bandwidth": 2300 }
}
EOF
    fi

    make_body "$d_call" "$l_call" >"$run/body.txt"
    station_env "$dialer"
    env "${STATION_ENV[@]}" "$DIAL_PAT" --config "$dial_cfg" --mbox "$SCRATCH/$dialer/mbox" \
        compose --p2p-only -s "$subject" "$l_call" <"$run/body.txt" >"$run/compose.log" 2>&1 ||
        echo "pat-loop: compose failed ($run/compose.log)" >&2

    local before_ovf before_ka
    before_ovf=$(count_both "Audio input buffer overflow")
    before_ka=$(count_both "Sending session KEEPALIVE")

    local start=$SECONDS rc=0
    env "${STATION_ENV[@]}" timeout "$TIMEOUT" "$DIAL_PAT" --config "$dial_cfg" \
        --mbox "$SCRATCH/$dialer/mbox" connect "varahf:///$l_call" \
        </dev/null >"$run/connect.log" 2>&1 || rc=$?
    local elapsed=$((SECONDS - start))
    # Give the listener a moment to file the message, then stop it.
    sleep 2
    stop "$listener_pid"
    reap "$listener_pid"
    # Reaped: drop it from PIDS, so cleanup does not signal a PID the OS may
    # have handed to another process since.
    local kept=() p
    for p in "${PIDS[@]}"; do
        if [[ $p != "$listener_pid" ]]; then
            kept+=("$p")
        fi
    done
    PIDS=("${kept[@]}")

    local ovf ka
    ovf=$(count_both "Audio input buffer overflow")
    ovf=$((ovf - before_ovf))
    ka=$(count_both "Sending session KEEPALIVE")
    ka=$((ka - before_ka))

    # The delivered body: the listener's inbox file carrying this subject,
    # text after the header's blank line, CRLF normalized (Pat stores CRLF).
    # No such file (nothing delivered) leaves match=no, which fails the run.
    local match=no received="" f
    for f in "$SCRATCH/$listener/mbox/$l_call/in/"*.b2f; do
        if [[ -f $f ]] && grep -q -- "Subject: $subject" "$f"; then
            received=$f
            break
        fi
    done
    if [[ -n "$received" ]]; then
        if python3 - "$received" "$run/body.txt" "$SELF_TEST_MISMATCH" >"$run/match" <<'EOF'
import sys
raw = open(sys.argv[1], "rb").read().replace(b"\r", b"")
got = raw.split(b"\n\n", 1)[1] if b"\n\n" in raw else b""
want = open(sys.argv[2], "rb").read()
if sys.argv[3] == "1":
    want = b"X" + want
print("yes" if got.rstrip(b"\n") == want.rstrip(b"\n") else "no")
EOF
        then
            match=$(cat "$run/match")
        else
            echo "pat-loop: the body comparison itself failed; counting it as no match" >&2
        fi
    fi

    local label="$d_call->$l_call"
    [[ $BODY_BYTES -gt 0 ]] && label="$label  ($(wc -c <"$run/body.txt") B body)"
    printf '%s  exit=%d  elapsed=%ds  body_match=%s  overflow_lines=%d  keepalives=%d\n' \
        "$label" "$rc" "$elapsed" "$match" "$ovf" "$ka"
    if [[ $rc -ne 0 || $match != yes || $ovf -ne 0 ]]; then
        FAILED=1
        LOGS+=("$run/connect.log" "$run/listener.log")
    fi
}

case "$DIRECTION" in
    a) run_direction a b ;;
    b) run_direction b a ;;
    both) run_direction a b; run_direction b a ;;
esac

if [[ $FAILED -eq 0 ]]; then
    echo PASS
else
    KEEP=1
    echo FAIL
    echo "pat-loop: logs kept in $SCRATCH:"
    for f in "${LOGS[@]}" "$SCRATCH/a/coppad.log" "$SCRATCH/b/coppad.log" "$SCRATCH/relay.log"; do
        echo "  $f"
    done
fi
# The exit status: 0 when every direction passed, 1 when any failed. This is
# a final test, not `exit`, because shellcheck 0.9.0 reads a top-level `exit`
# as making the EXIT trap's handler unreachable (SC2317 on `cleanup`).
[[ $FAILED -eq 0 ]]
