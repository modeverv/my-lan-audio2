# Timing JSONL schema 1

## Extended diagnostic fields

Records remain additive schema 1. `capture.peak` is optional amplitude only; it contains no
audio samples. `device_position_valid=false` excludes unavailable process-client positions
(this Windows implementation returned zero on every packet) and experimental KS cursor-mode
positions from device-gap/regression counters. `unavailable_device_positions` counts them.
The original short process smoke logs predate this correction and must not be interpreted as
thousands of genuine clock regressions.

`source_pulse` records both sides of the independent source's ReleaseBuffer call for a 20ms
440Hz pulse every 500ms. `pulse` records when capture finds an onset above 0.0001 following
at least 100ms worth of observed quiet frames. This is threshold detection, not encoded pulse
identity or acoustic calibration. `analyze-pulses.ps1` matches the latest preceding source
release within 250ms, rejects duplicates, and marks ambiguous pairings. Other audio, effects,
ring loss or a delay longer than one stimulus period can invalidate this estimate. Compare
matched counts as well as percentiles. A source block can contain the pulse at a nonzero
offset; the reported interval is submission of that containing block to availability of the
capture block, not a driver-free per-sample transit time.

`render_wake` and `source_end` describe the actual independent renderer callback periods and
frames, rather than the requested IAudioClient3 period alone. Source errors are reflected in
exit status and source_end.success; source startup parameters are in the sidecar source.log.

Direct KS startup includes requested and actual notification frames and actual cyclic-buffer
capacity. `kernel_position` contains the raw driver cursors. In legacy cursor mode, capture
records describe cursor deltas (variable frames), not an assumed half-buffer per notification.
`capture_end.audio_timeline_valid=false` and wrap-risk counters deliberately prevent claiming
lossless master chronology. A cursor only gives position modulo the ring, not a full cycle count.
The fallback has no reliable first-sample QPC timestamp: age percentiles are null. Driver
GETREADPACKET timestamps, when supported, are raw QPC ticks and are converted to 100ns.

`capture_to_record_us` in the optional signal probes includes the level/onset scan. In KS it
measures packet/cursor acquisition to the end of the in-memory scan; there is no WASAPI
ReleaseBuffer call. An event interval around 1ms is not proof of 1ms source-to-capture delay.
KS `discontinuities=0` is not a loss guarantee, especially with audio_timeline_valid=false.

Every line has `schema_version: 1`, `session_id` (string), and `event`.
Session IDs combine launch UNIX nanoseconds and PID; they are local run identifiers,
not cryptographically random IDs. No audio sample data or content is written.
File serialization is entirely outside the capture thread.

| Event | Meaning |
| --- | --- |
| `startup` | Endpoint, format, backend attempted/selected, fallback error, periods, tap status, MMCSS and requested duration |
| `capture` | One GetBuffer result: sequence, cumulative first_sample, raw device position, frames, flags and QPC timestamps |
| `wake` | One signaled WASAPI event: wake QPC, interval from previous event, total drained frames/packets and drain duration |
| `capture_end` | Authoritative capture-thread frame/packet/wake totals, CPU time, actual duration, initial/trailing no-event intervals and telemetry loss |
| `summary` | Reporter-observed distributions and counters, histogram bins, completion/error status |
| `error` | Initialization/capture/test-renderer failure with stage and message; exit status is nonzero |

`*_100ns` timestamps use Windows QPC expressed in 100ns units, **not raw QPC ticks or wall time**.
GetBuffer already returns QPC in these units. Locally read QPC is converted using
QueryPerformanceFrequency and u128 arithmetic before comparison. `*_us` durations are
integer microseconds, truncated toward zero. Cross-machine QPC clocks cannot be subtracted.

`capture_time_100ns` is immediately after GetBuffer. `first_sample_age_us` is the difference
between the post-ReleaseBuffer timestamp and the first-sample QPC reported by WASAPI.
It is a driver-timestamp-based estimate, not an acoustic end-to-end latency measurement.
The value is null when the timestamp-error flag is set or the driver timestamp is zero.
Negative values are retained in raw records and counted, but excluded from age percentiles.

`get_buffer_us` measures the GetBuffer call. `capture_to_record_us` measures acquisition through
ReleaseBuffer to the point immediately before constructing/enqueuing the timing record.
It does **not** measure PCM publication, UDP send, queue residence or disk I/O. A 0us result
can be less than the rounding resolution and does not mean zero work. `callback_drain_us`
includes all packet work and capture-record queue pushes for that wake, but not pushing the wake record.

Histograms and nearest-rank percentiles include the whole run (startup outliers are not removed).
`callback_interval_us` includes empty events. `nonempty_callback_interval_us` counts only events
that deliver packets and still includes source-idle gaps. `frames_per_callback` includes zero.
`time_to_first_wake_us` and `trailing_no_event_us` are separate because a wholly idle source may
not generate any callback intervals. An empty distribution is null, not zero latency.

An event gap of 500ms is not evidence of 500ms audio batching. Check BOTH
`packet_audio_duration_us` and the event's total frame count. An idle source or scheduler stall
can also create a long interval. `callbacks_containing_ge_500ms_audio` counts aggregate audio
per event; `audio_blocks_ge_500ms` counts individual WASAPI packets of that length.

WASAPI flags: 1 = discontinuity, 2 = silent, 4 = timestamp error. All discontinuities are counted;
`first_packet_discontinuities` identifies the startup subset. A discontinuity is not automatically
a capture-thread overrun; `capture_overrun_count` is null because the API does not expose a reliable
independent overrun counter. Device-position gaps are reported separately, without assigning cause.
They are computed only between consecutive telemetry sequences with valid timestamps.

The capture thread has authoritative totals independent of the telemetry queue. Reporter-derived
statistics are incomplete when `telemetry_dropped > 0`; avoid interpreting their gap counts as
complete audio-loss statistics. `complete_telemetry` reports this condition. CPU is GetThreadTimes
kernel+user time (coarse accounting); the percentage is relative to one logical core, excludes
initialization, reporter and optional renderer, and may round to zero on short runs.

The baseline logged `conventional loopback`. Extended endpoint runs log Default/Pre/Post and
the original/effective endpoint mute state. Pre requests options NONE; Post requests option 0x8.
Per-run pre_volume_pre_mute_verified stays false: a single run cannot certify mute independence.
Use paired runs and read-only KS capability replies for that assessment.

References:

- [Microsoft: GetBuffer timestamps, flags and buffer lifetime](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer)
- [Microsoft: event-driven loopback](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)
- [Microsoft: shared engine period initialization](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient3-initializesharedaudiostream)
- [Microsoft: driver tap-point capabilities and POST_VOLUME_LOOPBACK](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/ksproperty-audioloopback)

## macOS receiver events

The receiver writes additive JSONL `schema_version: 1` startup metadata and one-second cumulative summaries.
`receiver_start` includes actual output device format, requested playout/AV delay, gain and kernel timestamp setup status.
`receiver_summary` / `receiver_final` include arrivals, same-Windows-clock capture→send and send intervals,
Mac kernel queue time, callback interval/work time, presentation timestamp lead, invalid/foreign/queue drops,
and sample-ring/ASRC metrics. `receiver_stopped` confirms orderly shutdown; the final summary may be a
regular summary when stopping interactively between reporting intervals.

`--events` adds `receive` and `playout` metadata. No event contains PCM. `receive_time_ns` and `render_time_ns`
are relative to this receiver run's monotonic epoch; `session_id` is the **binary wire** session, suitable
for matching audio packet metadata. `playout.sequence` identifies the last available source packet in the
callback; `first_sample` is the source cursor at callback start, and `output_frames` belongs to the output clock.
Do not assume the full callback belongs to that one packet, especially across holes or ASRC boundaries.

`arrival_excess_ms` is change in receive time minus change in send-attempt QPC for adjacent arriving packets.
It measures interval expansion/contraction, **not absolute one-way latency**. Reorder may make it signed.
`kernel_queue_ms` is kernel receive timestamp→recvmsg completion on the same Mac. A negative value in an
individual event means unavailable; aggregate positive histograms omit unavailable values.
`interface_index` identifies the ingress interface (13/en8 in this experiment).
`presentation_lead_ms` is AUHAL's output host timestamp minus callback-entry host time; it is a reported
scheduling lead, not an acoustic DAC measurement. Do not add hardware buffer/device/safety values again
without accounting for overlap with this lead.

`fill_ms` is newest sample end minus playback cursor, including any holes. `missing_frames` counts output
frames zero-filled after startup; `underruns` counts affected callbacks, not network datagrams.
`receive_to_render_*` measures per available sample from kernel arrival to its assigned time in the render
block, excluding presentation lead. `startup_late_frames`, `skipped_source_frames`, `callback_discontinuities`
and `rebases` expose freshness recovery instead of silently growing latency.
`sender_clock_ppm` and `receiver_clock_ppm` are relative to Mac monotonic time; `drift_ppm` is their difference.
`asrc_ratio` includes the nominal input/output sample-rate ratio and the smoothly bounded correction.

Histograms report their resolution and saturation threshold; callback-work resolution is 0.001ms in the final
app and other intervals 0.05ms. Earlier result files used 0.05ms for callback work too, so their displayed zero
percentiles mean below rounding resolution. Exact observed maxima remain separately recorded.
Offline `scripts/analyze-receiver.py` computes sequence holes only when per-packet events are available;
incomplete logs/telemetry drops are not proof of network loss. `one_way_latency_ms` remains null.


### Receiver scheduling update

`receiver_scheduling` records requested mode, QoS/set/get status, whether the readback is the default
policy, and period/computation/constraint in milliseconds. A successful readback confirms configuration,
not a hard deadline guarantee. `handoff_batch_max_ms` aggregates the maximum packet queue wait in each
nonempty render callback. `handoff_packets` counts consumed packet descriptors. These separate socket
receipt wait from the subsequent audio-callback phase wait. See [A/B measurements](receiver-wakeup.md).
