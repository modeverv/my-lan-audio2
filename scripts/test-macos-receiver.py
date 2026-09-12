#!/usr/bin/env python3
"""Independent Python wire encoder -> real UDP/receiver integration, with no audio output."""
import argparse
import json
from pathlib import Path
import socket
import struct
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--network-scheduling", choices=["qos", "realtime"], default="qos")
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reserve:
    reserve.bind(('127.0.0.1', 0))
    port = reserve.getsockname()[1]
process = subprocess.Popen([str(root / 'target/release/receiver-macos'), '--network-scheduling',args.network_scheduling,'--diagnostic', '--bind', f'127.0.0.1:{port}', '--source', '127.0.0.1', '--seconds', '1'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
try:
    ready = json.loads(process.stdout.readline())
    assert ready['event'] == 'receiver_start', ready
    def packet(session, seq, first, acquired):
        header = struct.pack('<4sHHIHBBQQQIHHQQQ', b'LNAU', 1, 72, 1, 0, 2, 1, session, seq, first, 48000, 96, 0, acquired, acquired + 1, acquired + 2)
        return header + struct.pack('<192f', *([0.1, -0.1] * 96))
    old = packet(3, 0, 0, 100)
    new = packet(4, 0, 0, 200)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        destination = ('127.0.0.1', port)
        for value in [old, old, packet(3, 2, 192, 101), packet(3, 1, 96, 101), new, old]:
            sock.sendto(value, destination)
        for offset in (0, 4, 6, 13, 14, 15):
            bad = bytearray(new); bad[offset] = 255
            sock.sendto(bad, destination)
    stdout, stderr = process.communicate(timeout=5)
    assert process.returncode == 0, stderr
    final = next(row for row in map(json.loads, stdout.splitlines()) if row['event'] == 'receiver_final')
    policy=next(row for row in map(json.loads, stdout.splitlines()) if row['event']=='receiver_scheduling')
    assert policy['qos_status']==0,policy
    if args.network_scheduling=='realtime':
        assert policy['set_status']==0 and policy['get_status']==0 and not policy['is_default'],policy
        assert abs(policy['constraint_ms']-1.0)<0.001,policy
    assert final['received_packets'] == 5, final
    assert final['invalid_packets'] == 6, final
    assert final['retired_session_packets'] == 1, final
    assert final['playout']['duplicate_packets'] == 1, final
    assert final['playout']['reordered_packets'] == 1, final
    assert final['playout']['sessions'] == 2, final
    assert final['playout']['session_id'] == 4, final
    print('PASS: independent wire encoding, real UDP, invalid headers, duplicates, reorder, and retired sessions')
finally:
    if process.poll() is None:
        process.kill(); process.wait()
