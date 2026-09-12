#!/usr/bin/env python3
"""Exercise actual group membership + LNAU decoding without opening audio output."""
import json
import socket
import struct
import subprocess
import sys
import time

engine = sys.argv[1] if len(sys.argv) > 1 else "target/release/receiver-macos"
for multicast in (False, True):
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.bind(("0.0.0.0", 0))
        port = probe.getsockname()[1]
    args = [engine, "--diagnostic", "--bind", f"0.0.0.0:{port}",
            "--seconds", "2", "--network-scheduling", "qos"]
    if multicast:
        args += ["--multicast-group", "239.255.0.1",
                 "--multicast-interface", "127.0.0.1"]
    with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
        try:
            first = json.loads(process.stdout.readline())
            assert first["event"] == "receiver_start", first
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sender:
                sender.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton("127.0.0.1"))
                sender.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_LOOP, 1)
                destination = "239.255.0.1" if multicast else "127.0.0.1"
                for seq in range(30):
                    header = struct.pack("<4sHHIHBBQQQIHHQQQ", b"LNAU", 1, 72, 1, 1, 2, 1,
                                         987654321, seq, seq * 128, 48000, 128, 0,
                                         seq * 26667, seq * 26667, seq * 26667)
                    sender.sendto(header + bytes(128 * 2 * 4), (destination, port))
                    time.sleep(128 / 48000)
            stdout, stderr = process.communicate(timeout=8)
            assert process.returncode == 0, stderr
            records = [json.loads(line) for line in stdout.splitlines()]
            received = max(r.get("received_packets", 0) for r in records)
            assert received == 30, (received, records)
            print(f"{'multicast' if multicast else 'unicast'}: {received}/30 valid packets received")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
