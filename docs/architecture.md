# Architecture — Experiment 0

## Update: UDP sender

The optional WASAPI UDP path is now implemented in `apps/sender-windows/src/udp.rs`.
Chrome process-include is the selected capture backend for the current routing. See [selection and UDP measurements](udp-sender.md).
Before ReleaseBuffer, the capture thread copies available PCM directly into preallocated datagram slots (64 total).
It publishes through a bounded ArrayQueue and unparks competing workers; it never calls the network socket itself.
Workers use a connected nonblocking UDP socket and MMCSS Pro Audio, send immediately, recycle slots, and drop packets exceeding the configured acquisition-age deadline (default 5ms). The producer always advances sample/sequence indexes on pool exhaustion.
There is no deliberate batching wait, retransmission, resampling or DSP. Silence becomes explicit zero PCM. Only native float32 WASAPI is accepted; KS is rejected for UDP until its loss chronology is complete.
Fixed histograms record acquisition-to-publication, publication-to-send, acquisition-to-send and send-call duration. JSON serialization and joins occur after capture stops.
The implementation below describes the original diagnostic stage; its statements about not copying PCM apply only with UDP disabled (and without the optional level/pulse probes).

## Extended experiments (2026-09-12)

The implementation now also contains process include/exclude capture, PRE/POST requests,
temporary mute probes with restoration, an independent RAW/minimum-period source, pulse
matching, read-only KS topology traversal and a direct KS/WaveRT loopback backend.
See [the current backend findings](low-latency-backends.md). The baseline description below
describes the original timing-only path; optional level/pulse probes now read PCM in memory.
The process backend requests the endpoint-reference mix format with AUTOCONVERTPCM, rather
than assuming GetMixFormat or device positions are supported by the virtual process client.

Direct KS validates an output pin with the loopback category before opening it through
KsCreatePin. It requests a two-notification cyclic buffer, registers a driver event and
tries GETREADPACKET. Optional legacy position fallback reads every newly available frame
between cursor positions, including wrapping reads, using volatile loads. It records unknown
wrap risks and explicitly sets audio_timeline_valid=false. It is an experiment, not a complete
loss-accounting sender. It never installs a driver, replaces an APO, changes default devices,
or changes security settings. Driver pin lifetime owns the DMA buffer mapping.

Implemented workspace:

```text
apps/sender-windows  Windows-only event-driven WASAPI diagnostic executable
crates/telemetry     Non-real-time histogram/percentile aggregation, reusable later
scripts             Reproducible capture runs and JSONL summary extraction
```

The capture worker initializes COM (MTA), chooses a render endpoint, queries its native mix
format, and tries the requested shared-mode backend. Auto fallback reactivates the client
after failed initialization. The audio client and capture service remain on this worker.
The capture worker registers with MMCSS `Pro Audio`; failure is recorded without hiding it.

For each WASAPI event, the worker drains all available packets with GetBuffer/ReleaseBuffer,
assigns diagnostic sequence/sample counters and records timestamps. It never dereferences,
copies, converts, stores or transmits PCM. `AUDCLNT_BUFFERFLAGS_SILENT` therefore safely
supports a null PCM pointer. No sample-rate or endpoint-volume changes are performed.

Fixed-size records are pushed into a preallocated 32768-entry `ArrayQueue`. Overflow drops
telemetry and increments a counter; the capture thread never waits for the reporter.
This is a **telemetry-only queue**, not an audio buffer. There is no application PCM queue.
The ordinary reporter thread serializes JSONL, computes histograms and prints once per second.
All maps, JSON allocations and file I/O occur there. The queue is large enough to tolerate a
slow reporter without obscuring normal short runs; its overflow is always reported.

WaitForSingleObject drives capture. A bounded timeout only checks cancellation/duration;
it never triggers polling for audio. After an event all available packets are drained.
Ctrl+C sets a stop flag. RAII releases COM memory, stops clients, closes event handles and
reverts MMCSS. Reporter failures also stop capture and the optional tone worker.
Device removal produces an explicit error; automatic device switching is intentionally not
part of a fixed-device measurement.

The optional test renderer runs independently on the selected endpoint in shared mode.
It generates 440Hz f32 PCM at amplitude 0.005. Capture-thread CPU excludes this renderer.
Its startup occurs after capture initialization, so the initial no-event interval includes
renderer setup; it is reported separately from steady-state callback intervals.

## Remaining stages

This handoff specifically requests Experiment 0 first. Do not infer completion of the full
sender or network-audio project from this diagnostic executable.

1. Process include/exclude and independent-source comparisons are implemented. Continue on the
   actual production HDMI endpoint and validate source identity under real application routing.
2. PRE/mute retention is measured on Realtek and VB; verify behavior on the actual production
   HDMI/render endpoint. A successful
   conventional loopback initialization alone is not evidence of that behavior. Current telemetry
   deliberately reports conventional/unverified. Do not silently mute/unmute user output to test it.
3. Separate minimum-period/RAW render requests and direct KS small-buffer probes are implemented.
   Complete loss/overrun detection and cursor chronology before making KS a production sender.
4. Completed: the versioned UDP protocol, immediate bounded packetization, independent nonblocking
   network workers, deadline drops, publication/send timing and the Windows GUI.
5. Completed: localhost UDP measurements. Next: macOS diagnostic receive, receiver buffering,
   CoreAudio/RME output and drift/ASRC. See [the Mac handoff](MAC_HANDOFF.md).
