"""Command/data TCP proxy. CR lines are logged independently of packet boundaries."""
import asyncio
import json
import time


async def serve(port, target, log):
    async def client(reader, writer):
        try:
            remote, out = await asyncio.open_connection("127.0.0.1", target)
        except OSError:
            writer.close()
            return

        async def pump(src, dst, kind):
            pending = b""
            try:
                while data := await src.read(65536):
                    pending += data
                    while b"\r" in pending:
                        line, pending = pending.split(b"\r", 1)
                        log.write(json.dumps({"t": time.monotonic(), "kind": kind,
                                              "line": line.decode(errors="replace")}) + "\n")
                    dst.write(data)
                    await dst.drain()
            except (ConnectionError, OSError):
                pass
            finally:
                dst.close()
        await asyncio.gather(pump(reader, out, "host"), pump(remote, writer, "tnc"))
    return await asyncio.start_server(client, "127.0.0.1", port)
