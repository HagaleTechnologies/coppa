"""Clean-room black-box LinBPQ experiment. No LinBPQ source is used."""
import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

import fake_rigctld
import tee_proxy

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
LIN_HASH = "6b5be6ffc5c0c1304d3e4175958ce2c78967ac4e8463474ce29744dc8aee748d"
PACKAGES = [
    ("pool/main/g/glibc/libc6_2.39-0ubuntu8_i386.deb", "8928db539155135cc6943677d6ee158fe6380653188716e16daf9de7094e6476"),
    ("pool/main/g/gcc-14/libgcc-s1_14-20240412-0ubuntu1_i386.deb", "43cb16a9a1cbfcb90d86aeb445a1ab7ba60bd6d0acd44679ffad1c0dee11b10e"),
    ("pool/main/z/zlib/zlib1g_1.3.dfsg-3.1ubuntu2_i386.deb", "96bfd120cff8a4811154b80a177723d914dda190f295b857a931e924c978edc4"),
]


def fetch(url, path, expected=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        with urllib.request.urlopen(url, timeout=60) as src:
            path.write_bytes(src.read())
    got = hashlib.sha256(path.read_bytes()).hexdigest()
    if expected and got != expected:
        raise ValueError(f"sha256 mismatch for {path}: got {got}, expected {expected}")
    return got


def rewrite_config(text, ports):
    import re
    for key, value in ports.items():
        # ADDR and HAMLIB retain the loopback host and their own separator.
        if key == "ADDR":
            text = re.sub(r"(?m)^( ADDR 127\.0\.0\.1 )\d+$", rf"\g<1>{value}", text)
        elif key == "HAMLIB":
            text = re.sub(r"(?m)^( HAMLIB 127\.0\.0\.1:)\d+$", rf"\g<1>{value}", text)
        else:
            text = re.sub(rf"(?m)^( {key}=)\d+$", rf"\g<1>{value}", text)
    return text


def max_reply_delay(events):
    from collections import deque
    pending = deque()
    delays = []
    notices = ("PTT ", "SNR ", "BUFFER ", "IAMALIVE", "PENDING", "CANCELPENDING", "CONNECTED ", "DISCONNECTED")
    for e in events:
        if e["kind"] == "host":
            pending.append(e)
        elif e["kind"] == "tnc":
            line = e["line"]
            if line.startswith(notices):
                continue
            if line.startswith("BUSY ") and (not pending or pending[0]["line"] != "BUSY"):
                continue
            if line.startswith("VERSION ") and (not pending or pending[0]["line"] != "VERSION"):
                continue
            if pending:
                delays.append(e["t"] - pending.popleft()["t"])
    if pending:
        raise ValueError(f"unanswered commands: {list(pending)}")
    return max(delays, default=0)


def scan_hops(events, start, end):
    return sum(e["kind"] == "rig" and e["line"].startswith("F ") and start <= e["t"] <= end for e in events)


def ports(n):
    holders = []
    try:
        for _ in range(n):
            s = socket.socket()
            s.bind(("127.0.0.1", 0))
            holders.append(s)
        return [s.getsockname()[1] for s in holders]
    finally:
        for s in holders:
            s.close()


def pair():
    while True:
        p = ports(1)[0]
        if p == 65535 or p % 10 == 9:
            continue
        try:
            with socket.socket() as s:
                s.bind(("127.0.0.1", p + 1))
            return p
        except OSError:
            pass


async def experiment(args, scratch, lin_command, pat):
    processes = []
    logs = []
    servers = []
    a, b, proxy = pair(), pair(), pair()
    rig, http, telnet, fbb, pat_http = ports(5)
    def spawn(command, name, **kw):
        log = open(scratch / (name + ".log"), "w")
        logs.append(log)
        proc = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, **kw)
        processes.append(proc)
        return proc
    def events():
        return [json.loads(line) for line in (scratch / "wire.jsonl").read_text().splitlines()]
    async def until(predicate, timeout=60):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            result = predicate()
            if result:
                return result
            await asyncio.sleep(.1)
        raise TimeoutError("condition not reached; inspect " + str(scratch))
    async def reply_delay():
        # A live log can end between a command and its reply. Wait for the
        # whole burst before measuring; never discard an unanswered command.
        for _ in range(20):
            try:
                return max_reply_delay(events())
            except ValueError:
                await asyncio.sleep(.1)
        raise AssertionError("unanswered command after two seconds")
    async def pat_cmd(*cmd, body=None, timeout=400):
        proc = await asyncio.create_subprocess_exec(str(pat), "--config", str(scratch / "b/pat.json"),
            "--mbox", str(scratch / "b/mbox"), *cmd, env=envs[1], stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT)
        try:
            output, _ = await asyncio.wait_for(proc.communicate(body),timeout)
            with open(scratch / "pat.log", "ab") as out:
                out.write(output)
            return proc.returncode
        except BaseException:
            proc.kill()
            await proc.wait()
            raise
    envs = []
    bpq = None
    try:
        wire = open(scratch / "wire.jsonl", "w", buffering=1)
        logs.append(wire)
        servers.append(await fake_rigctld.serve(rig, wire))
        servers.append(await tee_proxy.serve(proxy, a, wire))
        # Data port needs transparent forwarding; binary data are never interpreted.
        async def data_client(reader, writer):
            try:
                remote, out = await asyncio.open_connection("127.0.0.1",a+1)
                async def pipe(src,dst):
                    try:
                        while data := await src.read(65536):
                            dst.write(data)
                            await dst.drain()
                    finally:
                        dst.close()
                await asyncio.gather(pipe(reader,out),pipe(remote,writer))
            except (OSError,ConnectionError):
                writer.close()
        servers.append(await asyncio.start_server(data_client,"127.0.0.1",proxy+1))
        for name, call, cmd, device in [("a","N0GW",a,"coppa-loop-A"),("b","N1CALL",b,"coppa-loop-B")]:
            d = scratch / name
            for sub in ["home","config","state/pat","data","mbox"]:
                (d/sub).mkdir(parents=True,exist_ok=True)
            (d/"home/.asoundrc").write_text((ROOT/"tools/pat-loop/asoundrc.in").read_text().replace("@DIR@",str(scratch)))
            (d/"coppad.toml").write_text(f'''[audio]
sample_rate = 48000
buffer_size = 16384
input_device = "{device}"
output_device = "{device}"
[radio]
ptt_method = "none"
[host]
bind_address = "127.0.0.1"
vara_enabled = true
vara_command_port = {cmd}
vara_data_port = {cmd+1}
[engine]
profile = "HF_STANDARD"
callsign = "{call}"
''')
            envs.append(dict(os.environ,HOME=str(d/"home"),XDG_CONFIG_HOME=str(d/"config"),XDG_STATE_HOME=str(d/"state"),XDG_DATA_HOME=str(d/"data")))
        spawn(["python3",str(ROOT/"tools/pat-loop/relay.py"),str(scratch),str(a),str(b)],"relay")
        await until(lambda:(scratch/"ab_in").exists())
        def daemon(i):
            name = "ab"[i]
            return spawn([str(args.coppad),"--config",str(scratch/name/"coppad.toml")],name+"-coppad",env=envs[i])
        gateway = daemon(0)
        caller = daemon(1)
        await asyncio.sleep(3)
        # LinBPQ's i386 libc cannot stat XFS/overlay inodes above 2**32.
        # tmpfs gives it small inode numbers; downloaded executables stay outside it.
        bpq = Path(tempfile.mkdtemp(prefix="coppa-bpq-",dir="/dev/shm"))
        (scratch/"bpq").symlink_to(bpq)
        (bpq/"bpq32.cfg").write_text(rewrite_config((HERE/"bpq32.cfg").read_text(),dict(ADDR=proxy,HAMLIB=rig,HTTPPORT=http,TCPPORT=telnet,FBBPORT=fbb)))
        # New-user mail is held by default. This lab has only synthetic
        # callsigns and needs local delivery visible to the destination.
        (bpq/"linmail.cfg").write_text('''main : {
 Streams = 10;
 BBSApplNum = 1;
 BBSName = "N0GW";
 SYSOPCall = "N0GW";
 DontHoldNewUsers = 1;
 MaxRXSize = 99999;
 MaxTXSize = 99999;
 Log_BBS = 1;
 Log_TCP = 1;
};
''')
        spawn(lin_command,"linbpq",cwd=bpq)
        await until(lambda:any(e["kind"]=="host" and e["line"]=="LISTEN ON" for e in events()))
        print("bpq-loop: "+(scratch/"linbpq.log").read_text(errors="replace").splitlines()[0],flush=True)
        (scratch/"b/pat.json").write_text(json.dumps(dict(mycall="N1CALL",locator="FN31PR",listen=[],http_addr=f"localhost:{pat_http}",version_reporting_disabled=True,varahf=dict(addr=f"localhost:{b}",bandwidth=2300))))
        (scratch/"b/state/pat/.account-confirmed_N1CALL.json").write_text(json.dumps(time.strftime("%Y-%m-%dT%H:%M:%SZ",time.gmtime())))
        print(f"bpq-loop: scratch {scratch}, ports A={a}/{a+1} B={b}/{b+1} rigctld={rig}",flush=True)
        async def session(label="session", detail=""):
            subject = f"bpq{os.getpid()}-{time.monotonic_ns()%1000000000}"
            body = "COP-172 gateway cable message"
            assert await pat_cmd("compose","-s",subject,"N0GW",body=(body+"\n").encode()) == 0
            begin = time.monotonic()
            result = await pat_cmd("connect","varahf:///N0GW-1")
            assert result == 0, f"Pat exited {result}"
            record = [e for e in events() if e["t"]>=begin]
            pending = next(e["t"] for e in record if e["line"]=="PENDING")
            connected = next(e for e in record if e["line"].startswith("CONNECTED N1CALL N0GW-1"))
            ended = next(e["t"] for e in record if e["line"]=="DISCONNECTED")
            stored = any(subject in f.read_text(errors="replace") and body in f.read_text(errors="replace") for f in bpq.glob("Mail/m_*.mes"))
            def bbs_log():
                return "\n".join(f.read_text(errors="replace") for f in bpq.glob("logs/*") if f.is_file())
            assert stored, "message not stored"
            await until(lambda:"B2 Msg To: N0GW" in bbs_log(),70)
            assert "Incoming Connect from N1CALL" in bbs_log(), "BBS connect log missing"
            assert scan_hops(record,pending,ended)==0,"scanner hopped in session"
            await until(lambda:any(e["kind"]=="rig" and e["line"].startswith("F ") and e["t"]>ended for e in events()),20)
            delay=await reply_delay()
            assert delay<1, f"reply delay {delay}"
            reader,writer=await asyncio.open_connection("127.0.0.1",telnet)
            transcript=bytearray()
            async def collect(seconds):
                end=time.monotonic()+seconds
                while time.monotonic()<end:
                    try:
                        data=await asyncio.wait_for(reader.read(65536),.2)
                    except asyncio.TimeoutError:
                        continue
                    if not data: break
                    transcript.extend(data)
                    # Refuse telnet options without treating them as text.
                    for i in range(len(data)-2):
                        if data[i]==255 and data[i+1] in (251,253):
                            writer.write(bytes([255,254 if data[i+1]==251 else 252,data[i+2]]))
                    await writer.drain()
            for command in [None,b"user\r",b"password\r",b"BBS\r",b"Sysop\r",b"L\r"]:
                if command: writer.write(command); await writer.drain()
                await collect(1)
            (scratch/"listing.txt").write_bytes(transcript)
            writer.close(); await writer.wait_closed()
            assert subject.encode() in transcript, "message absent from sysop L listing"
            print(f'{label} pat_exit=0 connected="{connected["line"]}" stored=yes listed=yes bbs_log=yes scan_hops_during_session=0 max_reply_ms={delay*1000:.1f} {detail}',flush=True)
        if args.scenario in ("session","all"):
            await session()
        if args.scenario in ("stall","all"):
            r,w=await asyncio.open_connection("127.0.0.1",b)
            await r.readuntil(b"\r")
            begin=time.monotonic()
            w.write(b"CONNECT N1CALL N0GW-1\r"); await w.drain()
            await until(lambda:any(e["line"]=="PENDING" and e["t"]>begin for e in events()))
            caller.send_signal(signal.SIGSTOP)
            await until(lambda:any(e["line"]=="CANCELPENDING" and e["t"]>begin for e in events()),50)
            caller.send_signal(signal.SIGCONT)
            w.close(); await w.wait_closed()
            cancelled=next(e["t"] for e in events() if e["line"]=="CANCELPENDING" and e["t"]>begin)
            await until(lambda:any(e["kind"]=="rig" and e["line"].startswith("F ") and e["t"]>cancelled for e in events()),20)
            assert await reply_delay()<1
            print("stall cancelpending=yes scan_resumed=yes max_reply_ms="+str(round((await reply_delay())*1000,1)),flush=True)
        if args.scenario in ("restart","all"):
            await asyncio.sleep(20)
            assert await pat_cmd("compose","-s","bpq-loop restart","N0GW",body=b"x"*6000+b"\n")==0
            monitor,monitor_writer=await asyncio.open_connection("127.0.0.1",b)
            begin=time.monotonic()
            connect=asyncio.create_task(pat_cmd("connect","varahf:///N0GW-1"))
            await until(lambda:any(e["line"].startswith("CONNECTED ") and e["t"]>begin for e in events()))
            async def outstanding_data():
                while True:
                    line=(await monitor.readuntil(b"\r")).decode().strip()
                    if line.startswith("BUFFER ") and int(line.split()[1])>0:
                        return
            # CONNECTED precedes the BBS banner. Wait for the caller's first
            # B2F write so this is an interrupted transfer, not an idle link
            # whose periodic keepalives can refresh its own idle timer.
            await asyncio.wait_for(outstanding_data(),60)
            monitor_writer.close(); await monitor_writer.wait_closed()
            restarted_at=time.monotonic()
            gateway.kill(); gateway.wait()
            gateway=daemon(0)
            await until(lambda:all(any(e["kind"]=="host" and e["t"]>restarted_at and e["line"].startswith(word) for e in events())
                                   for word in ["MYCALL ","BW2300","RETRIES 20","LISTEN ON"]))
            first=await connect
            assert first!=0,"dropped gateway should fail first Pat session"
            await session("restart",f"first_exit={first} startup_block_resent=yes second_exit=0")
        print("PASS",flush=True)
    finally:
        for proc in reversed(processes):
            if proc.poll() is None:
                proc.send_signal(signal.SIGCONT)
                proc.terminate()
        for proc in processes:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill(); proc.wait()
        for server in servers:
            server.close(); await server.wait_closed()
        for log in logs:
            log.close()
        if bpq is not None and not args.keep:
            shutil.rmtree(bpq)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coppad",type=Path,default=ROOT/"target/release/coppad")
    parser.add_argument("--linbpq",type=Path)
    parser.add_argument("--expect-linbpq-sha256")
    parser.add_argument("--pat",type=Path)
    parser.add_argument("--cache",type=Path,default=Path(os.environ.get("XDG_CACHE_HOME",str(Path.home()/".cache")))/"coppa-bpq-loop")
    parser.add_argument("--scenario",choices=["session","stall","restart","all"],default="all")
    parser.add_argument("--keep",action="store_true")
    parser.add_argument("--allow-debug",action="store_true")
    args=parser.parse_args()
    args.cache=args.cache.expanduser().resolve()
    scratch=Path(tempfile.mkdtemp(prefix="bpq-loop.", dir="/tmp"))
    try:
        if sys.platform != "linux":
            raise ValueError("Linux only: the audio loop uses ALSA")
        if not args.coppad.is_file():
            raise ValueError("build release coppad with --features cpal-backend")
        args.coppad=args.coppad.resolve()
        if "/target/debug/" in str(args.coppad) and not args.allow_debug:
            raise ValueError("refusing debug coppad; use --allow-debug")
        lin=args.linbpq.resolve() if args.linbpq else args.cache/"linbpq"
        got=fetch("https://www.cantab.net/users/john.wiseman/Downloads/linbpq",lin,args.expect_linbpq_sha256)
        lin.chmod(0o755)
        print(f"bpq-loop: LinBPQ sha256 {got}",flush=True)
        if got!=LIN_HASH:
            print(f"WARNING: recorded LinBPQ sha256 is {LIN_HASH}",flush=True)
        if Path("/lib/ld-linux.so.2").exists() and subprocess.run(["/lib/ld-linux.so.2","--list",str(lin)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode == 0:
            lin_command=[str(lin)]
        else:
            runtime=args.cache/"i386"
            for url,digest in PACKAGES:
                deb=args.cache/Path(url).name
                fetch("https://archive.ubuntu.com/ubuntu/"+url,deb,digest)
                subprocess.run(["dpkg-deb","-x",str(deb),str(runtime)],check=True)
            lin_command=[str(runtime/"usr/lib/ld-linux.so.2"),"--library-path",str(runtime/"usr/lib/i386-linux-gnu"),str(lin)]
        pat=args.pat.resolve() if args.pat else Path(subprocess.check_output(["bash","-c",'source "$1"; SCRATCH=$2; die() { echo "$*" >&2; exit 2; }; fetch_pat',"bash",str(ROOT/"tools/pat-loop/fetch-pat.sh"),str(scratch)],text=True).strip())
        asyncio.run(experiment(args,scratch,lin_command,pat))
        return 0
    except TimeoutError as exc:
        print(f"FAIL: {exc}",flush=True)
        return 1
    except (ValueError,OSError,subprocess.SubprocessError) as exc:
        print(f"bpq-loop: setup error: {exc}",flush=True)
        return 2
    except (AssertionError,TimeoutError) as exc:
        print(f"FAIL: {exc}",flush=True)
        return 1
    finally:
        if args.keep:
            print(f"bpq-loop: kept {scratch}",flush=True)
        else:
            shutil.rmtree(scratch)


if __name__=="__main__":
    raise SystemExit(main())
