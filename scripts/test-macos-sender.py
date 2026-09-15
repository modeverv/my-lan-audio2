#!/usr/bin/env python3
"""Validate real sender datagrams using an independent decoder and the Rust receiver."""
import json
from pathlib import Path
import socket
import struct
import subprocess

root = Path(__file__).resolve().parents[1]
sender = root / 'dist/LAN Audio Sender.app/Contents/MacOS/LAN Audio Sender'
with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
    sock.bind(('127.0.0.1', 0)); sock.settimeout(3)
    port = sock.getsockname()[1]
    subprocess.run([str(sender), '--test-packets', '127.0.0.1', str(port)], check=True)
    first = 0
    session = None
    for seq, frames in enumerate([128, 128, 44, 1]):
        data = sock.recv(2048)
        h = struct.unpack('<4sHHIHBBQQQIHHQQQ', data[:72])
        assert h[:4] == (b'LNAU', 1, 72, 1), h
        assert h[4:7] == (2 if seq == 0 else 1 if seq == 3 else 0, 2, 1), h
        if session is None: session = h[7]
        assert h[7:13] == (session, seq, first, 48000, frames, 0), h
        assert 0 < h[13] <= h[14] <= h[15], h
        assert len(data) == 72 + frames * 8
        pcm = struct.unpack('<' + 'f' * frames * 2, data[72:])
        assert pcm == tuple(([0., 0.] if seq == 3 else [0.25, -0.5]) * frames)
        first += frames
    subprocess.run([str(sender), '--test-packets', '127.0.0.1', str(port)], check=True)
    assert struct.unpack_from('<Q', sock.recv(2048), 16)[0] != session

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as reserve:
    reserve.bind(('127.0.0.1', 0)); port = reserve.getsockname()[1]
process = subprocess.Popen([str(root / 'target/release/receiver-macos'), '--diagnostic', '--bind', f'127.0.0.1:{port}', '--source', '127.0.0.1', '--seconds', '1'], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
try:
    assert json.loads(process.stdout.readline())['event'] == 'receiver_start'
    subprocess.run([str(sender), '--test-packets', '127.0.0.1', str(port)], check=True)
    stdout, stderr = process.communicate(timeout=5)
    assert process.returncode == 0, stderr
    final = next(row for row in map(json.loads, stdout.splitlines()) if row['event'] == 'receiver_final')
    assert final['received_packets'] == 4 and final['invalid_packets'] == 0, final
    assert final['playout']['sessions'] == 1, final
finally:
    if process.poll() is None: process.kill(); process.wait()
for host, port in [('bad', '40100'), ('127.0.0.1', '0'), ('127.0.0.1', '70000')]:
    assert subprocess.run([str(sender), '--test-packets', host, port]).returncode != 0
print('PASS: packet splitting, PCM, silence, timestamps, session restart, invalid address, and Rust receiver interoperability')
