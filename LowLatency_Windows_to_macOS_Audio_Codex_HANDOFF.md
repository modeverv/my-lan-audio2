# Low-Latency Windows → macOS Network Audio
## Codex Handoff / Implementation Specification

**Project goal:**  
Windows PCで再生されるシステム全体の音声（Chrome / YouTube / YouTube Music / Spotify / OS音声など）を、同一有線LAN上のmacOSへ極小レイテンシーで送り、macOS側からRME UCX IIへ出力する。

**Primary requirement:**  
「音が鳴ること」よりも、**送り出しレイテンシーを最小化し、かつ各段階の遅延を数値で説明できること**を優先する。

**Background / previous failure:**  
以前の実装では安定動作のために約100ms程度の遅延が必要だった。  
本来は同一スイッチ配下の有線LANを想定しており、10ms級を期待していた。  
前回はWindows / Voicemeeter / capture API / sender / receiver / outputの各段でどこに遅延が積まれていたか十分に観測できず、原因を特定できなかった。  
今回は**最初からtelemetryを設計に含め、どこで何ms消費しているかを必ず可視化する。**

---

# 1. Scope

## In scope

- Windows sender
- WASAPI system loopback capture
- UDP PCM transport
- macOS receiver
- timestamp/sample-counter based playout
- small receiver-side jitter/playout buffer
- packet reorder
- late packet discard
- missing packet handling
- sender/receiver clock drift estimation
- ASRCによる微小なクロック追従
- CoreAudio output to RME
- configurable AV sync delay
- end-to-end telemetry
- offline simulator
- automated tests
- latency analyzer

## Out of scope for initial implementation

- Dante/AES67互換
- encryption
- internet/WAN transport
- Wi-Fi optimization
- codec compression
- many-to-many routing
- GUI polish
- auto device discovery
- redundant NIC paths
- professional-grade PLC
- sub-millisecond deterministic transport

---

# 2. Target Environment

## Sender

- Windows 11
- dedicated projector/media PC
- Apps may include:
  - Chrome
  - YouTube
  - YouTube Music PWA
  - Spotify
  - other system audio
- Capture requirement:
  - **all system audio as one mixed stream**
- Voicemeeter Banana should NOT be required.

## Receiver

- macOS
- RME UCX II
- output through CoreAudio

## Network

- wired Ethernet
- same LAN
- normally one switch between nodes
- multicast is not required initially
- UDP unicast is sufficient

---

# 3. Core Design Principles

## 3.1 Sender must not intentionally buffer audio

The sender should behave conceptually as:

```text
WASAPI event
    ↓
GetBuffer()
    ↓
assign sample counter / timestamp / sequence
    ↓
publish immediately
    ↓
UDP
```

Do NOT create a 20ms / 40ms / 100ms sender-side audio ring buffer.

The sender prioritizes **freshness over guaranteed delivery**.

---

## 3.2 The receiver owns the only intentional time buffer

Application-level intentional playout buffering should exist only on the receiver.

Initial target:

```text
receiver playout/jitter buffer = 5ms
```

Make it configurable.

Recommended options:

```text
2ms
5ms
10ms
20ms
```

Do not create multiple stacked timing buffers.

Kernel buffers, WASAPI/CoreAudio buffers, NIC queues etc. naturally exist, but the application must avoid adding redundant buffering stages.

---

## 3.3 Sender timeline is master

The Windows sender owns the application audio timeline.

Use a monotonically increasing **sample counter** as the canonical timebase.

For example at 48kHz:

```text
master_time_seconds = sample_counter / 48000.0
```

Every UDP audio packet must carry the first sample position represented by that packet.

The macOS receiver must place incoming data according to this timeline, not by arrival order.

---

## 3.4 Late data has no value

For real-time playback:

> correct audio arriving after its playout deadline is useless.

Do not delay newer audio while trying to deliver old data.

If a packet is too old to meet the receiver playout deadline, it may be dropped.

---

## 3.5 Reordering must not collapse the timeline

Example:

```text
packet A
packet B missing
packet C arrives
```

The receiver must preserve:

```text
AAAA | XXXX | CCCC
```

Never shift C earlier to hide the gap.

---

# 4. Proposed Monorepo

Prefer one monorepo so one Codex agent can reason about both operating systems and the shared protocol.

Suggested structure:

```text
network-audio/
├── Cargo.toml
├── README.md
├── docs/
│   ├── architecture.md
│   ├── protocol.md
│   ├── telemetry.md
│   └── experiments.md
│
├── crates/
│   ├── audio-protocol/
│   ├── sender-core/
│   ├── receiver-core/
│   ├── telemetry/
│   ├── simulator/
│   └── latency-analyzer/
│
├── apps/
│   ├── sender-windows/
│   └── receiver-macos/
│
├── scripts/
└── testdata/
```

Rust is preferred unless a concrete technical blocker appears.

Reasons:

- shared protocol/core implementation
- strong type system
- predictable memory handling
- good cross-platform support
- suitable for lock-free / real-time-adjacent code
- Codex can work on a single workspace

Windows-specific code may use `windows-rs`.

macOS-specific code may use CoreAudio FFI or an appropriate maintained Rust wrapper.

Do not introduce a large framework before the core timing path works.

---

# 5. Common Audio Block Representation

Internal common representation:

```rust
struct AudioBlock {
    first_sample: u64,
    frame_count: u32,
    channels: u16,
    sample_rate: u32,
    // interleaved native float samples unless proven otherwise
    samples: ...
}
```

Avoid format conversions in the sender hot path where possible.

Initial strategy:

- use the native Windows mix format during early experiments
- if the system mix format is 48kHz float32 stereo, transport it as-is
- if it is 44.1kHz, first make the system work at 44.1kHz before adding SRC
- later optionally standardize to 48kHz

---

# 6. Windows Sender

## 6.1 Capture source

Use **WASAPI system loopback**.

Goal:

```text
Chrome / Spotify / YouTube / system sounds
                 ↓
          Windows Audio Engine
                 ↓
           WASAPI Loopback
                 ↓
              Sender
```

Do NOT require Voicemeeter.

Prefer event-driven capture.

### Required Windows capture experiments

1. **Actively test `IAudioClient3`.**
   - Query the shared-mode engine periods actually supported by the selected render endpoint.
   - Attempt to use the smallest stable period rather than assuming the legacy/default period.
   - Log both the requested and actually observed callback period.
   - If `IAudioClient3` is unavailable or cannot be used for this loopback path, fall back cleanly and record the reason in telemetry.

2. **Prefer/test a PRE-VOLUME / PRE-MUTE loopback tap on supported Windows 11 systems.**
   - Investigate `AUDIOLOOPBACK_TAPPOINT_PREVOLUMEMUTE` on Windows 11 24H2+.
   - The desired capture point is the mixed system PCM **before endpoint master volume and mute are applied**, so the physical HDMI/projector output may be muted or set to zero without silencing the network stream.
   - Detect support at runtime; do not assume every endpoint/driver supports the tap.
   - If unavailable, fall back to conventional WASAPI system loopback and clearly report that the capture may be affected by endpoint volume/mute.
   - Record the selected tap mode in telemetry:
     `pre-volume/pre-mute`, `conventional loopback`, or other fallback.

3. **Benchmark process-loopback capture against endpoint-loopback capture for latency.**
   - Implement/test Windows process loopback using `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK`.
   - Specifically test `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE` with the sender process as the excluded target, so the sender can capture essentially all other system audio without binding capture to one physical render endpoint.
   - Compare this backend against:
     - conventional endpoint WASAPI loopback,
     - endpoint loopback using `IAudioClient3` with the smallest stable shared-mode period,
     - PRE-VOLUME / PRE-MUTE endpoint loopback where supported.
   - Do **not** assume process loopback is faster. Choose the production backend based on measurement.
   - For every backend, measure and report at least:
     - observed callback interval p50/p95/p99/max,
     - frames per callback,
     - maximum callback gap,
     - discontinuities,
     - estimated first-sample age at capture publication where measurable,
     - capture-to-publish latency,
     - CPU cost.
   - Run the same test material and duration for each backend.
   - Record the winning backend and the evidence in `docs/experiments.md`.

Do not assume the callback period. Measure it.

---

## 6.2 Capture telemetry is mandatory

At startup display/log:

```text
device name
mix format
sample rate
channels
sample format
default engine period
minimum supported period
actual callback interval
frames per callback
```

Runtime statistics:

```text
callback interval p50
callback interval p95
callback interval p99
callback interval max

frames/callback min
frames/callback max

capture discontinuity count
capture overrun count
```

This is critical because a previous implementation may have appeared to deliver audio in roughly 0.5-second chunks.

If that phenomenon occurs again, the program must make it immediately obvious whether it originated at WASAPI or later.

---

# 7. Sender Hot Path

The critical path should do as little work as possible.

Concept:

```text
WASAPI capture event
        ↓
read available frames
        ↓
assign:
  stream_id
  session_id
  sequence
  first_sample
        ↓
publish packet descriptor
        ↓
return
```

Avoid:

- heap allocation
- logging directly from hot path
- file I/O
- blocking locks
- sleep/poll loops
- resampling
- DSP
- large memcpy chains

Use preallocated memory.

---

# 8. Sender Network Workers

The initial implementation should support a configurable worker count:

```text
1
2
3
```

Default to **1 worker first**, then measure.

The design should allow testing whether multiple sender workers reduce scheduling outliers.

Possible structure:

```text
                 ┌──────── worker A → UDP
capture/publish ─┼──────── worker B → UDP
                 └──────── worker C → UDP
```

Do NOT make multiple workers mandatory if one worker is already stable.

---

## 8.1 Worker semantics

Workers must prefer fresh data.

Packets should have a maximum send age/deadline.

Conceptually:

```text
latest published seq = 1000
worker next seq       = 990
```

The worker must not spend time delivering hopelessly stale data.

Use **deadline-aware drop**.

Do not implement unconditional “latest wins” unless measurements justify it.

Normal case:

```text
100
101
102
103
```

All packets should be transmitted.

Abnormal backlog case:

```text
receiver deadline makes packets 100..104 useless
```

Those may be dropped so newer data can proceed.

---

## 8.2 UDP send

Use UDP.

Investigate non-blocking or completion-based sending.

Do not block the capture thread on network transmission.

If network transmission unexpectedly stalls, it must not stall WASAPI capture.

A tiny preallocated SPSC/multi-reader publication structure is acceptable **only as decoupling**, not as intentional audio buffering.

Normal queue/publication lag should be approximately zero or one packet.

---

# 9. Packetization / MTU

Avoid IP fragmentation.

**Sender MUST NOT wait to fill MTU. If an audio block is already MTU-safe, transmit it immediately regardless of payload efficiency. MTU is a maximum payload constraint, not a packetization target.**

Assume Ethernet MTU 1500 unless detected/configured otherwise.

For 48kHz float32 stereo:

```text
128 frames
× 2 channels
× 4 bytes
= 1024 bytes PCM
```

This fits comfortably in one UDP datagram with protocol headers.

If WASAPI delivers a larger block, do not wait.

Immediately split the already-available block into MTU-safe packets.

Example:

```text
WASAPI gives 480 frames

→ packet 1: 128 frames
→ packet 2: 128 frames
→ packet 3: 128 frames
→ packet 4: 96 frames
```

All should be emitted immediately.

---

# 10. UDP Protocol v1

Keep the protocol deliberately small and versioned.

Suggested header:

```rust
struct AudioPacketHeader {
    magic: u32,
    protocol_version: u16,

    stream_id: u32,
    session_id: u64,

    sequence: u64,
    first_sample: u64,

    sample_rate: u32,
    frame_count: u16,
    channels: u8,
    sample_format: u8,

    flags: u16,
}
```

Payload:

```text
interleaved PCM frames
```

Initial sample format:

```text
float32 little-endian
```

or the native mix format if there is a strong reason.

Document all byte order and compatibility rules.

---

# 11. Receiver Architecture

Concept:

```text
UDP receive
    ↓
timestamp/sample-index placement
    ↓
single playout/jitter buffer
    ↓
missing packet handling
    ↓
clock-drift correction / ASRC
    ↓
CoreAudio render callback
    ↓
RME UCX II
```

---

# 12. Receiver Playout Buffer

Initial default:

```text
5ms
```

At 48kHz:

```text
5ms = 240 frames
```

Make configurable.

Receiver behavior:

- accept out-of-order packets
- place them by `first_sample`
- wait until playout deadline
- if missing data is still absent at deadline, declare it lost
- do not wait indefinitely
- discard packets arriving after their playout deadline

Metrics:

```text
current fill ms
target fill ms
minimum fill ms
maximum fill ms

late packets
reordered packets
duplicate packets
missing packets
underruns
```

---

# 13. Missing Packet Handling

Loss concealment is separate from resampling.

Initial implementation can be deliberately simple.

Suggested progression:

### Phase 1

```text
zero fill
```

### Phase 2

short crossfade / interpolation for very small gaps

### Phase 3

optional better PLC if required

Do not delay the core transport project for advanced PLC.

---

# 14. Clock Drift

Windows sender clock and RME/macOS output clock will not be exactly identical.

Example:

```text
sender: 48000.6 Hz
receiver hardware: 47999.8 Hz
```

Without compensation, the receiver buffer will slowly grow or drain.

Do NOT solve this by making the buffer huge.

Estimate clock drift from long-term buffer behavior / sender sample timeline.

Metrics:

```text
estimated sender rate
estimated receiver rate
clock drift ppm
ASRC ratio
```

---

# 15. ASRC

ASRC is for **clock drift correction**, not packet-loss concealment.

The correction should be very small and smooth.

Example:

```text
ratio 1.000000
→ 1.000008
→ 1.000011
```

Avoid sudden resampling-ratio jumps.

Make the ASRC implementation replaceable.

Evaluate candidate libraries based on:

- latency
- quality
- real-time safety
- licensing
- Rust integration
- variable ratio support

Do not commit to a heavy implementation before the basic no-ASRC transport is measurable.

---

# 16. CoreAudio Output

Receiver must render through CoreAudio to the selected RME output.

Requirements:

- no sleep-based audio loop
- CoreAudio callback drives consumption
- callback reads exactly the requested frames
- hot path avoids allocation and blocking
- output device and buffer size are logged

Log:

```text
device
sample rate
buffer frames
buffer duration ms
render callback interval
render callback jitter
underrun count
```

---

# 17. AV Sync Delay

The real-world use case is projector playback.

Video path:

```text
Windows
  ↓ HDMI
Projector
  ↓ scaling / processing / display
```

Audio path:

```text
Windows
  ↓ LAN
macOS
  ↓ RME
speakers
```

The projector may itself introduce tens of milliseconds of video latency.

Therefore the receiver should expose:

```text
AV_SYNC_DELAY_MS
```

Range suggestion:

```text
0 .. 250ms
```

Important distinction:

```text
base transport latency
+
intentional AV sync delay
=
actual output delay
```

Telemetry must report these separately.

The transport must remain low-latency even when intentional AV delay is added.

---

# 18. Telemetry Schema

Use a shared telemetry format on Windows and macOS.

Recommended:

```text
JSON Lines
```

Every experiment/run must have a common:

```text
session_id
```

Every audio packet has:

```text
session_id
stream_id
sequence
first_sample
```

This allows sender/receiver logs to be joined.

---

## 18.1 Windows sender event examples

```json
{
  "event": "capture",
  "session_id": 123,
  "sequence": 456,
  "first_sample": 100000,
  "frames": 128,
  "capture_time_ns": 123456789
}
```

```json
{
  "event": "send",
  "session_id": 123,
  "sequence": 456,
  "send_start_ns": 123456900,
  "send_end_ns": 123456980
}
```

---

## 18.2 macOS receiver examples

```json
{
  "event": "receive",
  "session_id": 123,
  "sequence": 456,
  "first_sample": 100000,
  "receive_time_ns": 223456789
}
```

```json
{
  "event": "playout",
  "session_id": 123,
  "sequence": 456,
  "first_sample": 100000,
  "buffer_depth_frames": 240
}
```

Do not perform heavy JSON serialization on the real-time hot path.

Collect compact binary/fixed-size telemetry events into a non-blocking queue and serialize outside the critical thread if necessary.

---

# 19. Latency Analyzer

Create a tool such as:

```bash
cargo run -p latency-analyzer -- \
  sender.jsonl \
  receiver.jsonl
```

Output:

```text
Session
-------------------------
duration

Windows Capture
-------------------------
callback p50
callback p95
callback p99
callback max
frames/callback

Sender
-------------------------
publish latency p50/p99/max
send syscall p50/p99/max
sender backlog max
deadline drops

Network / Arrival
-------------------------
arrival interval p50/p99/max
reorder count
loss count

Receiver
-------------------------
buffer fill p50/min/max
late packets
underruns

Clock
-------------------------
estimated drift ppm
ASRC ratio min/max

Output
-------------------------
CoreAudio callback p50/p99/max
output buffer frames

Total
-------------------------
estimated base audio latency
intentional AV sync delay
```

Important:

Cross-machine wall clocks are not initially assumed to be synchronized well enough for one-way latency.

For early versions, use:

- packet sequence behavior
- sender sample timeline
- receiver playout timeline
- buffer depth
- round-trip tests where needed

Later, optional application-level clock sync can be added.

---

# 20. Simulator

The simulator is mandatory.

It must allow the core sender/receiver logic to be tested without two physical machines.

Pipeline:

```text
fake audio source
      ↓
sender-core
      ↓
network simulator
      ↓
receiver-core
      ↓
fake audio sink
```

Inject configurable:

```text
fixed network delay
random jitter
packet loss
packet duplication
packet reordering
sender clock ppm offset
receiver clock ppm offset
scheduler stalls
burst delays
```

Example test profile:

```text
delay = 0.3ms
jitter = ±0.2ms
loss = 0.01%
reorder = 0.01%
sender clock = +12ppm
receiver clock = -5ppm
```

Assertions:

- receiver stays near target buffer fill
- no unbounded buffer growth
- no unbounded drain
- late packets are discarded correctly
- sequence wrap/long runtime is safe
- drift estimator converges
- ASRC ratio remains bounded
- packet reorder does not shift timeline

---

# 21. Performance Telemetry UI

A full GUI is not needed initially.

A terminal/TUI is sufficient.

Sender should continuously show something like:

```text
Windows Sender
--------------------------------
Format              48000 / 2ch / f32
WASAPI period       2.67 ms
Callback p99        2.73 ms
Callback max        3.20 ms
Frames/callback     128

Publish p99         0.010 ms
Send p99            0.080 ms
Send max            0.310 ms
Backlog             0 packets
Deadline drops      0
Packets/sec         375
```

Receiver:

```text
macOS Receiver
--------------------------------
Target buffer       5.00 ms
Current buffer      5.08 ms
Buffer min/max      4.61 / 5.42 ms

Late packets        0
Reordered           2
Missing             0
Underruns           0

Clock drift         +8.1 ppm
ASRC ratio          1.0000081

CoreAudio buffer    64 frames
AV sync delay       32.0 ms
```

The previous project failed partly because “the system is delayed” was observable but “where the delay is” was not.

This project must never regress to that state.

---

# 22. Experiment Plan

Implement in this order.

## Experiment 0: Windows capture only

No UDP.

```text
system audio
↓
WASAPI loopback
↓
telemetry only
```

Measure:

- callback interval
- frames/callback
- max callback gap
- discontinuities

Acceptance:

- explain the actual Windows capture period
- prove or disprove the previous “~0.5 second chunk” observation

Do not continue until this is understood.

---

## Experiment 1: Windows sender → localhost receiver

```text
WASAPI
↓
UDP localhost
↓
dummy receiver
```

Measure sender publication and UDP behavior.

Acceptance:

- sender adds no large buffering
- backlog normally 0 or 1 packet
- no unexplained tens-of-ms stalls

---

## Experiment 2: Windows → macOS network, no audio output

```text
Windows WASAPI
↓
UDP LAN
↓
macOS receiver
↓
dummy sink
```

Measure:

- arrival pattern
- reorder
- packet loss
- jitter
- receiver buffer behavior

---

## Experiment 3: macOS CoreAudio output

Add:

```text
receiver
↓
CoreAudio
↓
RME
```

Initial playout buffer:

```text
20ms
```

Then reduce:

```text
10ms
5ms
2ms
```

Measure stability at each setting.

---

## Experiment 4: clock drift

Run for:

```text
30 min
1 hour
several hours
```

without ASRC first.

Observe buffer trend.

Then enable drift estimation and ASRC.

Acceptance:

- buffer remains bounded near target
- no periodic insert/drop hacks in normal operation

---

## Experiment 5: projector A/V synchronization

Use a test video with a visual flash and audio click.

Measure perceived or recorded A/V offset.

Set:

```text
AV_SYNC_DELAY_MS
```

to align audio with projector output.

Keep this separate from transport latency.

---

# 23. Latency Targets

Do not claim success based only on “sounds okay”.

## Stage A

```text
base audio latency < 20ms
stable
zero underruns in normal LAN operation
```

## Stage B

```text
base audio latency ≈ 10ms class
stable
```

## Stage C

```text
attempt ≈ 5ms class
```

Sub-5ms is experimental and not required for initial success.

Important:

The exact physical DAC latency is difficult to infer purely from software timestamps.

Where possible, perform hardware loopback measurement using:

```text
RME OUT
→ cable
→ RME IN
```

and/or a synchronized impulse/flash test.

---

# 24. Acceptance Criteria

The project is not considered successful merely because audio plays.

It is successful when all of the following are true:

1. Windows system audio reaches macOS/RME.
2. Voicemeeter is not required.
3. Sender does not intentionally accumulate tens of milliseconds of audio.
4. Receiver has one explicit playout/jitter buffer.
5. Packet reorder does not shift the audio timeline.
6. Late packets are discarded.
7. Missing samples are handled deterministically.
8. Sender/receiver clock drift is measurable.
9. Drift can be compensated without a large buffer.
10. Every major latency component is observable.
11. The previous ~100ms requirement can be explained or eliminated.
12. The previous suspected ~500ms capture chunk phenomenon can be reproduced/explained or disproven.
13. A 20ms-class configuration is stable first.
14. Then 10ms-class operation is evaluated.
15. AV sync delay is independent from transport latency.

---

# 25. Real-Time Coding Rules

For hot paths:

- no heap allocation after initialization where practical
- no blocking mutexes
- no logging to disk
- no console output
- no sleeps
- no polling when an event-driven API exists
- no resampling on sender
- no arbitrary batching
- no “wait until N ms of PCM has accumulated”
- avoid copying audio repeatedly
- use preallocated packet buffers
- collect telemetry with fixed-size/non-blocking events

Correctness and observability are more important than micro-optimization outside the hot path.

---

# 26. Important Hypotheses to Test

Do not assume these are true. Instrument and test them.

## H1

Previous ~100ms latency was caused by stacked application buffers.

## H2

Voicemeeter contributed significant latency.

## H3

Previous sender/capture code may have accumulated large chunks before sending.

## H4

Windows WASAPI loopback itself can provide sufficiently frequent capture events for <20ms transport.

## H5

Same-switch Ethernet contributes negligible latency relative to OS/audio buffering.

## H6

Clock drift, rather than network jitter, may have forced the old implementation to use a large buffer.

## H7

A 5ms receiver playout buffer may be sufficient under normal wired LAN conditions.

Each hypothesis should have a concrete experiment and telemetry evidence.

---

# 27. Do Not Prematurely Optimize

Do not begin with:

- custom kernel driver
- Windows APO
- ASIO virtual driver
- FPGA
- RTP/AES67
- PTP
- custom NIC driver
- complex FEC

First prove the basic user-space architecture.

Only escalate if measurement shows a concrete bottleneck.

---

# 28. Deliverables

Initial Codex implementation should produce:

```text
1. buildable monorepo
2. Windows WASAPI loopback diagnostic app
3. Windows UDP sender
4. macOS UDP diagnostic receiver
5. macOS CoreAudio/RME receiver
6. protocol documentation
7. simulator
8. latency analyzer
9. telemetry schema
10. experiment README
```

Also provide:

```text
README.md
docs/architecture.md
docs/protocol.md
docs/experiments.md
```

---

# 29. First Codex Task

Start with **Experiment 0 only**.

Do not immediately implement the entire system.

Implement a Windows program that:

1. enumerates the default render endpoint
2. opens WASAPI system loopback in event-driven mode
3. captures all system audio
4. does NOT write audio to disk
5. records only lightweight timing statistics
6. reports:
   - mix format
   - engine/device period
   - callback interval histogram
   - frames per callback
   - maximum callback gap
   - discontinuities
7. can run for several minutes
8. prints a final summary

Critical question:

> Does Windows deliver loopback audio in small regular blocks, or is there actually a large (~500ms) batching phenomenon in this environment?

Only after this result is known should the UDP sender be implemented.

---

# 30. Guiding Principle

The project should be treated as a measurable real-time distributed audio system.

The guiding rule is:

> **Do not hide timing problems with a larger buffer. Measure them, identify the responsible stage, and fix that stage.**

Sender:

```text
capture
→ timestamp
→ immediate best-effort delivery
```

Receiver:

```text
small bounded wait
→ deadline decision
→ clock correction
→ output
```

Everything should be designed around preserving that model.

---

# 31. Windows progress — 2026-09-12

Chrome capture selection and the initial UDP sender are implemented in this workspace.
Use process-include with the Chrome parent PID for the current VB-Audio routing.
The final PRE/process-exclude/minimum-period/POST comparison did not establish a better usable route;
VB direct KS opened but delivered no notifications or PCM during the Chrome test.
This is an implementation choice for the measured environment, not a Windows latency lower bound.

The sender copies available f32 PCM into a preallocated bounded pool, immediately splits to MTU-safe
datagrams, and uses 1..3 nonblocking MMCSS network workers. Default is one worker and a 5ms send-age deadline.
In the 32-second one-worker localhost test, all 12,792 datagrams arrived; acquisition-to-send-call
p50/p99/max were 0.0835/0.1776/0.2898ms. This excludes Chrome's upstream generation/capture latency.
The Node diagnostic receive side had larger scheduling outliers and is not a production audio receiver.

Read `docs/udp-sender.md` for selection evidence and all worker comparisons, and `docs/protocol.md`
plus `docs/protocol-v1-golden.hex` before implementing the macOS receiver.
Network PCM is opt-in with `--udp-to IP:PORT`; no LAN destination was used in these tests.
Next: macOS diagnostic receive, sample-index placement, session reset/loss/reorder handling,
then the single receiver buffer and Core Audio output. No macOS output or LAN end-to-end latency is claimed.
