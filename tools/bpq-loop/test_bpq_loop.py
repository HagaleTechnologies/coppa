"""Offline checks for the real-program experiment; no third-party modules."""
import importlib.util
import contextlib
import io
import tempfile
from unittest import mock
from pathlib import Path
import sys
import unittest

# unittest -I deliberately removes cwd; load only this directory's modules.
HERE = Path(__file__).resolve().parent
for name in ("fake_rigctld", "tee_proxy", "bpq_loop"):
    spec = importlib.util.spec_from_file_location(name, HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
from bpq_loop import max_reply_delay, scan_hops, rewrite_config
from fake_rigctld import reply


class Checks(unittest.TestCase):
    def test_reply_delay_skips_notices(self):
        events = [dict(t=0,kind="host",line="LISTEN ON"),dict(t=.1,kind="tnc",line="BUSY OFF"),dict(t=1.2,kind="tnc",line="OK")]
        self.assertEqual(max_reply_delay(events),1.2)
        self.assertGreater(max_reply_delay(events),1)
        with self.assertRaises(ValueError):
            max_reply_delay(events[:1])

    def test_hops(self):
        events = [dict(t=4,kind="rig",line="F 7100000")]
        self.assertEqual(scan_hops(events,5,10),0)
        self.assertEqual(scan_hops(events,3,10),1)

    def test_config_ports_only(self):
        original = (HERE/"bpq32.cfg").read_text()
        changed = rewrite_config(original,dict(ADDR=9000,HAMLIB=9001,HTTPPORT=9002,TCPPORT=9003,FBBPORT=9004))
        expected = original.replace("8300","9000").replace("4532","9001").replace("8080","9002").replace("8010","9003").replace("8011","9004")
        self.assertEqual(changed,expected)

    def test_rig_replies(self):
        for command in ["F 7100000","M USB 2400","T 1"]:
            self.assertEqual(reply(command),"RPRT 0\n")
        for command in ["f","m","t","v"]:
            self.assertNotIn("RPRT",reply(command))

    def test_relative_cache_stays_valid_after_child_changes_directory(self):
        import bpq_loop
        with tempfile.TemporaryDirectory() as directory:
            base=Path(directory)
            binary=base/"fake-binary"
            binary.write_bytes(b"test")
            async def experiment(args, scratch, command, pat):
                self.assertEqual(args.cache,base/"cache")
                self.assertTrue(Path(command[0]).is_absolute())
                if "--library-path" in command:
                    self.assertTrue(Path(command[command.index("--library-path")+1]).is_absolute())
                self.assertTrue(Path(command[-1]).is_absolute())
            argv=["bpq-loop","--coppad",str(binary),"--linbpq",str(binary),"--pat",str(binary),"--cache","cache"]
            with contextlib.chdir(base),mock.patch.object(sys,"argv",argv), \
                 mock.patch.object(bpq_loop,"fetch",return_value=bpq_loop.LIN_HASH), \
                 mock.patch.object(bpq_loop.subprocess,"run",return_value=mock.Mock(returncode=1)), \
                 mock.patch.object(bpq_loop,"experiment",side_effect=experiment), \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(bpq_loop.main(),0)


class StalledCallerCleanup(unittest.TestCase):
    """The stall scenario must not hand the next scenario a live dial."""

    @staticmethod
    def run_against(before, on_abort, timeout):
        import asyncio
        import bpq_loop
        received = []
        async def scenario():
            async def tnc(reader, writer):
                writer.write(before)
                received.append(await reader.readuntil(b"\r"))
                writer.write(on_abort)
                await writer.drain()
                with contextlib.suppress(ConnectionError):
                    await reader.read()
                writer.close()
            server = await asyncio.start_server(tnc, "127.0.0.1", 0)
            try:
                r, w = await asyncio.open_connection("127.0.0.1", server.sockets[0].getsockname()[1])
                try:
                    await bpq_loop.end_caller_dial(r, w, timeout)
                finally:
                    w.close(); await w.wait_closed()
            finally:
                server.close(); await server.wait_closed()
        asyncio.run(scenario())
        return received

    def test_live_dial_is_aborted_and_its_disconnect_awaited(self):
        received = self.run_against(b"OK\rPTT OFF\r", b"OK\rDISCONNECTED\r", 5)
        self.assertEqual(received, [b"ABORT\r"])

    def test_dial_the_reaper_already_ended_is_accepted(self):
        self.assertEqual(self.run_against(b"DISCONNECTED\r", b"OK\r", 5), [b"ABORT\r"])

    def test_no_disconnect_fails_instead_of_continuing(self):
        with self.assertRaises(TimeoutError):
            self.run_against(b"OK\r", b"OK\r", .5)
