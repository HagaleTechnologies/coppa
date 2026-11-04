#!/usr/bin/env python3
"""Real RNS 1.5.7 client for the COP-138 userspace audio loop."""
import sys
import time
import RNS

config, me, peer = sys.argv[1:]
r = RNS.Reticulum(configdir=config)
identity = RNS.Identity()
expected = bytes(i % 251 for i in range(300))
dest = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                       "cop138", "probe", me)


def received(data, packet):
    print(f"RECEIVED DATAGRAM {len(data)} bytes ok={data == expected}", flush=True)


dest.set_packet_callback(received)


class AnnounceHandler:
    aspect_filter = f"cop138.probe.{peer}"

    def received_announce(self, destination_hash, announced_identity, app_data):
        out = RNS.Destination(announced_identity, RNS.Destination.OUT,
                              RNS.Destination.SINGLE, "cop138", "probe", peer)
        RNS.Packet(out, expected).send()
        print("SENT DATAGRAM 300 bytes", flush=True)


RNS.Transport.register_announce_handler(AnnounceHandler())
# Offset the stations' announces so they do not repeatedly collide.
time.sleep(2 if me == "a" else 12)
while True:
    dest.announce()
    print("ANNOUNCED", flush=True)
    time.sleep(20)
