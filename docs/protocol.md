# UDP audio protocol v1

Explicit byte encoding, independent of Rust struct layout. Integers and interleaved float32 PCM are little endian.
Maximum UDP datagram is 1400 bytes, including the 72-byte application header. This fits IPv4/IPv6 with ordinary headers at MTU 1500; lower path MTUs/tunnels require a smaller `--udp-frames` value.
Default packet limit is 128 frames. An already available 480-frame block becomes 128+128+128+96 immediately. Short blocks are sent as-is; no fill timer or accumulation.

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 4 | ASCII `LNAU` |
| 4 | 2 | version = 1 |
| 6 | 2 | header length = 72 |
| 8 | 4 | stream_id, CLI default 1 |
| 12 | 2 | flags, below |
| 14 | 1 | channels (1..8) |
| 15 | 1 | sample_format = 1: IEEE float32 LE |
| 16 | 8 | session_id |
| 24 | 8 | sequence, per datagram |
| 32 | 8 | first_sample, per interleaved frame |
| 40 | 4 | sample_rate, Hz |
| 44 | 2 | frame_count, nonzero |
| 46 | 2 | reserved, zero |
| 48 | 8 | capture QPC, normalized to 100ns; GetBuffer completion |
| 56 | 8 | publication preparation QPC, 100ns; after PCM copy, before header/queue push |
| 64 | 8 | send-attempt QPC, 100ns; immediately before UDP API call |
| 72 | frame_count × channels × 4 | PCM payload |

Flags: bit 0 = silent (payload still contains explicit zero samples); bit 1 = first datagram of session;
bit 2 = source timestamp-error flag; bit 3 = source data-discontinuity flag; bit 4 = source device position unavailable/untrusted.
All other bits are reserved zero. Flags 2/3 describe the captured block and can appear on all its fragments.
The first-session flag is a hint; the first datagram can be dropped. A receiver must use session_id, not depend on receiving that flag.

Session ID starts from the low 64 bits of an OS-generated GUID and increments on capture discontinuity, timestamp-error flag, or a detected endpoint device-position jump. Sequence and first_sample then restart at zero. A receiver must reset its timeline on a new session and reject late packets from retired sessions. Multiple workers can reorder datagrams even across a session boundary.

Process loopback does not expose a trustworthy device position in this implementation. Its first_sample counts delivered frames; reported discontinuities trigger a new session. Unreported upstream loss cannot be reconstructed and this is not a claim of a hardware sample clock. Endpoint mode additionally checks consecutive device positions.
Sender pool exhaustion, deadline drops and send failures advance sequence/sample counters without changing session, leaving explicit holes for the receiver. Never concatenate across those holes.

All three QPC fields refer to the Windows sender clock, not UTC, not the original Chrome JS generation instant, and not the sample's physical presentation time. They cannot be directly subtracted from a Mac timestamp without clock calibration. first_sample is the receiver's placement key; receiver drift correction remains to be implemented.
The send-attempt timestamp does not prove transmission on the wire. A successful UDP send means the local stack accepted the datagram, not remote delivery.

Receiver validation: check magic/version/header length, reserved bits, supported format/channels/rate, frame count and exact payload size before accessing samples. Track sequence holes, duplicates, reorder and session changes. The localhost diagnostic receiver checks these and does not play or save PCM.

UDP is enabled only by explicit `--udp-to IP:PORT`. One connected, nonblocking socket is shared by 1..3 competing workers; all use the same source port. There is no handshake, authentication, encryption, retry, FEC or discovery in v1. This stage is for the requested LAN experiment.

The JSONL diagnostic schema_version remains 1 and is separate from this wire format. Its textual session_id is a log-run identifier, not the binary audio session_id.
The [shared golden datagram](protocol-v1-golden.hex) contains one stereo frame (1.0, -0.5), rate 48000, stream 1, session 3, sequence 4, first_sample 5 and QPC values 6/7/8. Rust encoding and the Node probe check the same fixture. The exact header layout, packet splitting, exact PCM preservation, silence, pool overflow, deadline dropping and discontinuity restart have dedicated tests. Node's independent decoder validates real datagrams in `scripts/udp-loopback-test.mjs`.

## macOS sender clock

The macOS sender uses the same v1 layout. Its three timestamp fields are CoreAudio host time converted to 100 ns units (capture completion, preparation, send attempt), not Windows QPC or UTC. Clock calibration is still required for cross-machine comparisons. It increments the session on input sample-time discontinuity or recovery from an input render error. Its first_sample counts delivered input frames; device sample time is used only to detect discontinuities.
