"""Test-only rigctld stand-in. Logs commands on the proxy's monotonic clock."""
import asyncio
import json
import time


def reply(line):
    op = line.split()[0] if line.split() else ""
    if op.startswith("+"):
        return {"+f": "get_freq:\nFrequency: 7100000\nRPRT 0\n", "+m": "get_mode:\nMode: USB\nPassband: 2400\nRPRT 0\n"}.get(op,"RPRT 0\n")
    return {"f": "7100000\n", "m": "USB\n2400\n", "t": "0\n", "v": "VFOA\n"}.get(op, "RPRT 0\n")


async def serve(port, log):
    async def client(reader, writer):
        try:
            while raw := await reader.readline():
                line = raw.decode().strip()
                log.write(json.dumps({"t": time.monotonic(), "kind": "rig", "line": line}) + "\n")
                writer.write(reply(line).encode())
                await writer.drain()
        except ConnectionError:
            pass
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass
    return await asyncio.start_server(client, "127.0.0.1", port)
