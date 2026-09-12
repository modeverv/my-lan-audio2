# WASAPI render-boundary probe (Windows x64)

Finite, opt-in experiment for the explicitly selected PID. Not a production capture backend.
Copies PCM before the original `IAudioRenderClient::ReleaseBuffer`, then publishes the copy
after the original call returns. A separate collector reads the shared-memory slot and records
timings, format, frame count and peak. No PCM is written to disk or sent over the network.

`Initialize`, `InitializeSharedAudioStream` and `GetService` supply actual stream formats;
`GetBuffer` tracks the borrowed pointer. Unknown formats/buffers are counted and skipped.
The original API arguments, return values, playback destination and sample data are preserved.
Multiple producers use try-locks; contention or a full ring drops telemetry instead of waiting.
The fixed ring has 128 slots, each with a maximum 32 KiB PCM block.

## Build on this PC

Requires the already-installed VS 18 BuildTools x64 toolchain. Detours v4.0.1 is MIT licensed
and fetched from Microsoft's repository; its license stays in `target/detours/LICENSE.md`.

```powershell
git clone --depth 1 --branch v4.0.1 https://github.com/microsoft/Detours.git target\detours
# Expected commit: e4bfd6b03e50de46b47abfbd1e46b384f0c5f833
cmd /c '"C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat" >nul && cd /d D:\work\my-lan-audio2\target\detours\src && nmake /nologo'
.\scripts\build-render-tap.cmd
```

Do not overwrite the DLL while a tested application still has it loaded. After collector-only
changes, `.\scripts\build-render-tap.cmd collector` builds just the EXE.

## Run

```powershell
# Owned source sanity check, inside one process; NOT the interprocess latency benchmark.
.\target\render-tap\render-probe.exe 0 8 D:\work\my-lan-audio2\target\render-tap\render-tap.dll
# Selected app: pause sufficiently for the old stream to close, attach, then resume.
# Replace 3280 with the verified audio-producing PID on the current machine.
.\target\render-tap\render-probe.exe 3280 30 D:\work\my-lan-audio2\target\render-tap\render-tap.dll > runs\new-name.jsonl
```

Use a new output filename; shell redirection itself can overwrite files.
Maximum session duration is 600 seconds. The session ends independently inside the target,
even if the collector exits. An error, no packets, or an incomplete run returns a nonzero exit.
Consequently a paused negative-control run with zero calls is expected to return nonzero.

The collector uses MMCSS Pro Audio where available and buffers metadata until capture ends
to avoid disk I/O on the receive path. `entry_to_collector_us` includes the copy, original
ReleaseBuffer call and IPC scheduling. It is measured from a fully prepared render block;
it excludes the application's decoding, DSP, earlier buffering, and any network/receiver latency.
`release_interval_us` is the block submission interval, a separate metric.

## Lifetime and limitations

- Hooks remain installed but PCM collection is disabled after the finite session. The DLL
  and pass-through trampolines remain until the target exits. They are not forcibly unloaded
  while another audio thread could still be returning through them. Restart the target to
  remove them completely. No registry, startup, driver or security settings are modified.
- Run only one collector against a PID at a time. Reuse the same DLL for subsequent sessions;
  do not load differently named builds into a process with an existing probe.
- The initial stream must be created after attachment to discover its exact format. Existing
  untracked streams are skipped. Formats are kept in a bounded 256-entry diagnostic table;
  long-lived churn and arbitrary concurrent stream lifecycles require additional hardening.
- This is x64 only. Function addresses are resolved from matching system DLL paths in the
  target, not hardcoded addresses. Access/load failures are reported; no sandbox or signing
  policy is disabled. Chrome's audio utility process rejected the DLL on this PC.
- Per-block peaks support float32 and PCM16; other encodings are not decoded for peak analysis.
  The owned self-test assumes this machine's float32 native mix format.
- This is not transparent per-tab capture, synchronized multi-stream mixing, or a long-duration
  reliability test. It does not establish an operating-system latency guarantee.

See [actual app results](../../docs/live-app-capture.md).
