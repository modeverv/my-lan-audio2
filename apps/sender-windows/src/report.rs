use crate::{
    Options,
    capture::{self, Record, Shared},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    sync::{Arc, atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use telemetry::Histogram;

#[derive(Default)]
struct Stats {
    callbacks: Histogram,
    active_callbacks: Histogram,
    frames_per_wake: Histogram,
    frames_per_packet: Histogram,
    packet_duration_us: Histogram,
    age_us: Histogram,
    get_buffer_us: Histogram,
    record_us: Histogram,
    drain_us: Histogram,
    discontinuities: u64,
    first_packet_discontinuities: u64,
    timestamp_errors: u64,
    negative_ages: u64,
    silent_frames: u64,
    packet_count: u64,
    total_frames: u64,
    device_position_gap_frames: u64,
    device_position_regressions: u64,
    previous_end: Option<u64>,
    previous_active_wake: Option<u64>,
    gaps_ge_100ms: u64,
    gaps_ge_500ms: u64,
    blocks_ge_500ms: u64,
    callback_audio_ge_500ms: u64,
    peak_max: f32,
    level_packets: u64,
    nonzero_packets: u64,
    invalid_device_positions: u64,
}

impl Stats {
    fn record(&mut self, record: Record, sample_rate: u64) {
        match record {
            Record::Udp { .. }
            | Record::SourcePulse { .. }
            | Record::Pulse { .. }
            | Record::RenderWake { .. }
            | Record::KernelPosition { .. } => {}
            Record::Capture {
                sequence,
                frames,
                flags,
                device_position,
                device_position_valid,
                first_sample_age_us,
                get_buffer_us,
                capture_to_record_us,
                peak,
                ..
            } => {
                self.packet_count += 1;
                if let Some(peak) = peak {
                    self.level_packets += 1;
                    self.peak_max = self.peak_max.max(peak);
                    self.nonzero_packets += u64::from(peak > 0.000001);
                }
                self.total_frames += frames as u64;
                self.frames_per_packet.record(frames as u64);
                let duration = frames as u64 * 1_000_000 / sample_rate;
                self.packet_duration_us.record(duration);
                self.blocks_ge_500ms += u64::from(duration >= 500_000);
                self.get_buffer_us.record(get_buffer_us);
                self.record_us.record(capture_to_record_us);
                if flags & 1 != 0 {
                    self.discontinuities += 1;
                    if sequence == 0 {
                        self.first_packet_discontinuities += 1;
                    }
                }
                if flags & 2 != 0 {
                    self.silent_frames += frames as u64;
                }
                if flags & 4 != 0 {
                    self.timestamp_errors += 1;
                }
                match first_sample_age_us {
                    Some(age) if age >= 0 => self.age_us.record(age as u64),
                    Some(_) => self.negative_ages += 1,
                    None => {}
                }
                // Only compare consecutive received records; telemetry loss would
                // otherwise be mislabelled as an audio-device gap.
                if device_position_valid {
                    if let Some(end) = self.previous_end {
                        if device_position >= end {
                            self.device_position_gap_frames += device_position - end;
                        } else {
                            self.device_position_regressions += 1;
                        }
                    }
                    self.previous_end = Some(device_position + frames as u64);
                } else {
                    self.invalid_device_positions += 1;
                    self.previous_end = None;
                }
            }
            Record::Wake {
                time_100ns,
                interval_us,
                frames,
                packets,
                drain_us,
            } => {
                if let Some(interval) = interval_us {
                    self.callbacks.record(interval);
                    self.gaps_ge_100ms += u64::from(interval >= 100_000);
                    self.gaps_ge_500ms += u64::from(interval >= 500_000);
                }
                if packets != 0 {
                    if let Some(previous) = self.previous_active_wake {
                        self.active_callbacks.record((time_100ns - previous) / 10);
                    }
                    self.previous_active_wake = Some(time_100ns);
                }
                self.frames_per_wake.record(frames);
                self.callback_audio_ge_500ms +=
                    u64::from(frames * 1_000_000 / sample_rate >= 500_000);
                self.drain_us.record(drain_us);
            }
        }
    }

    fn summary(&self) -> Value {
        json!({"event":"summary","callback_interval_us":self.callbacks.summary(),
            "peak_max":self.peak_max,"level_packets":self.level_packets,"nonzero_packets":self.nonzero_packets,
            "nonempty_callback_interval_us":self.active_callbacks.summary(),
            "frames_per_callback":self.frames_per_wake.summary(),"frames_per_packet":self.frames_per_packet.summary(),
            "packet_audio_duration_us":self.packet_duration_us.summary(),"first_sample_age_us":self.age_us.summary(),
            "get_buffer_us":self.get_buffer_us.summary(),"capture_to_record_us":self.record_us.summary(),
            "callback_drain_us":self.drain_us.summary(),"discontinuities":self.discontinuities,
            "first_packet_discontinuities":self.first_packet_discontinuities,
            "timestamp_errors":self.timestamp_errors,"negative_first_sample_ages":self.negative_ages,
            "silent_frames":self.silent_frames,"observed_packets":self.packet_count,"observed_frames":self.total_frames,
            "device_position_gap_frames":self.device_position_gap_frames,"device_position_regressions":self.device_position_regressions,
            "unavailable_device_positions":self.invalid_device_positions,
            "capture_overrun_count":Value::Null,"capture_overrun_note":"No reliable overrun count; KS cursor mode also cannot resolve whole-ring wraps. See capture_end.audio_timeline_valid and cursor gap counters.",
            "callback_gaps_ge_100ms":self.gaps_ge_100ms,"callback_gaps_ge_500ms":self.gaps_ge_500ms,
            "audio_blocks_ge_500ms":self.blocks_ge_500ms,
            "callbacks_containing_ge_500ms_audio":self.callback_audio_ge_500ms,
            "callback_histogram_us":self.callbacks.bins(),"frames_per_callback_histogram":self.frames_per_wake.bins()})
    }

    fn print_progress(&self) {
        let interval = self.callbacks.summary();
        eprintln!(
            "packets={} callback p50/p99/max={:?}/{:?}/{:?} us, frames/wake={:?}..{:?}, discontinuities={}, >=500ms blocks={}",
            self.packet_count,
            interval.p50,
            interval.p99,
            interval.max,
            self.frames_per_wake.summary().min,
            self.frames_per_wake.summary().max,
            self.discontinuities,
            self.blocks_ge_500ms
        );
    }
}

struct StopOnDrop(Arc<Shared>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop.store(true, Ordering::Relaxed);
    }
}

pub fn run(options: Options) -> Result<()> {
    if options.udp_to.is_some() {
        anyhow::ensure!(
            !options.tone_only && !options.ks_inspect && !options.list_devices,
            "UDP requires an active capture backend"
        );
        anyhow::ensure!(
            !matches!(options.backend, crate::Backend::Ks),
            "KS loss chronology is incomplete; UDP is supported only with WASAPI capture"
        );
    }
    if options.ks_inspect {
        return capture::inspect_ks(options.device.as_deref());
    }
    if options.list_devices {
        return capture::list_devices();
    }
    let session_id = format!(
        "{:x}-{:x}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id()
    );
    let mut file = options
        .output
        .as_ref()
        .map(|path| {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(BufWriter::new)
                .with_context(|| format!("create telemetry {}", path.display()))
        })
        .transpose()?;
    let mut emit = |mut value: Value| -> Result<()> {
        value["session_id"] = json!(session_id);
        value["schema_version"] = json!(1);
        if let Some(file) = file.as_mut() {
            serde_json::to_writer(&mut *file, &value)?;
            file.write_all(b"\n")?;
        }
        Ok(())
    };
    let shared = Arc::new(Shared::new(32768));
    if options.control_stdin {
        let control = shared.clone();
        thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(line) if line.trim() != "stop" => continue,
                    _ => break,
                }
            }
            control.stop.store(true, Ordering::Relaxed);
        });
    }
    let signal = shared.clone();
    ctrlc::set_handler(move || signal.stop.store(true, Ordering::Relaxed))?;
    if options.tone_only {
        let result = thread::scope(|scope| -> Result<()> {
            let shared_source = shared.clone();
            let opt = &options;
            let handle = scope.spawn(move || {
                capture::tone(
                    opt.device.clone(),
                    shared_source,
                    opt.render_period_frames,
                    opt.render_raw,
                    opt.seconds,
                    opt.pulse_probe,
                )
            });
            let _stop = StopOnDrop(shared.clone());
            let mut intervals = Histogram::default();
            let mut frames = Histogram::default();
            loop {
                while let Some(record) = shared.queue.pop() {
                    if let Record::RenderWake {
                        interval_us,
                        frames: count,
                    } = record
                    {
                        if let Some(us) = interval_us {
                            intervals.record(us);
                        }
                        frames.record(count as u64);
                    }
                    emit(serde_json::to_value(record)?)?;
                }
                if handle.is_finished() && shared.queue.is_empty() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let result = handle
                .join()
                .map_err(|_| anyhow::anyhow!("source thread panicked"))?;
            emit(
                json!({"event":"source_end","render_callback_interval_us":intervals.summary(),"render_frames_per_callback":frames.summary(),"telemetry_dropped":shared.dropped.load(Ordering::Relaxed),"success":result.is_ok()}),
            )?;
            result
        });
        if let Some(file) = file.as_mut() {
            file.flush()?;
        }
        return result;
    }
    thread::scope(|scope| -> Result<()> {
        let (startup_tx, startup_rx) = mpsc::channel();
        let shared_capture = shared.clone();
        let capture_options = &options;
        let capture_handle =
            scope.spawn(move || capture::run(capture_options, &shared_capture, startup_tx));
        // This drops before the scope joins workers, including on reporter I/O errors.
        let _stop_on_error = StopOnDrop(shared.clone());
        let metadata = match startup_rx.recv() {
            Ok(metadata) => metadata,
            Err(_) => {
                let result = capture_handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("capture thread panicked"))?;
                if let Err(error) = &result {
                    emit(
                        json!({"event":"error","stage":"initialize","requested_backend":format!("{:?}",options.backend),"message":format!("{error:#}")}),
                    )?;
                }
                return result.map(|_| ());
            }
        };
        let sample_rate = metadata["format"]["sample_rate"]
            .as_u64()
            .context("missing sample rate")?;
        eprintln!("{}", serde_json::to_string_pretty(&metadata)?);
        emit(metadata.clone())?;
        let tone_handle = options.test_tone.then(|| {
            eprintln!(
                "Playing quiet 440 Hz test tone (-46 dBFS); endpoint settings are unchanged."
            );
            let shared = shared.clone();
            let device_id = metadata["device_id"].as_str().map(str::to_owned);
            let (period, raw, seconds) = (
                options.render_period_frames,
                options.render_raw,
                options.seconds + 2,
            );
            let pulses = options.pulse_probe;
            scope.spawn(move || capture::tone(device_id, shared, period, raw, seconds, pulses))
        });
        let mut stats = Stats::default();
        let mut last_progress = Instant::now();
        let mut last_sequence = None;
        loop {
            // Limit each drain so error/termination checks also run during sustained load.
            for _ in 0..4096 {
                let Some(record) = shared.queue.pop() else {
                    break;
                };
                if let Record::Capture { sequence, .. } = record {
                    if last_sequence.is_some_and(|last| sequence != last + 1) {
                        stats.previous_end = None;
                    }
                    last_sequence = Some(sequence);
                }
                stats.record(record, sample_rate);
                emit(serde_json::to_value(record)?)?;
            }
            if capture_handle.is_finished() && shared.queue.is_empty() {
                break;
            }
            if tone_handle.as_ref().is_some_and(|h| h.is_finished()) {
                shared.stop.store(true, Ordering::Relaxed);
            }
            if last_progress.elapsed() >= Duration::from_secs(1) {
                stats.print_progress();
                last_progress = Instant::now();
            }
            // Only this ordinary reporter sleeps. Capture and render wait on WASAPI events.
            thread::sleep(Duration::from_millis(10));
        }
        let end_result = capture_handle
            .join()
            .map_err(|_| anyhow::anyhow!("capture thread panicked"))?;
        shared.stop.store(true, Ordering::Relaxed);
        let tone_result = match tone_handle {
            Some(handle) => handle
                .join()
                .map_err(|_| anyhow::anyhow!("tone thread panicked"))?,
            None => Ok(()),
        };
        let mut summary = stats.summary();
        summary["telemetry_dropped"] = json!(shared.dropped.load(Ordering::Relaxed));
        summary["complete_telemetry"] = json!(shared.dropped.load(Ordering::Relaxed) == 0);
        summary["capture_success"] = json!(end_result.is_ok());
        summary["test_tone_success"] = json!(tone_result.is_ok());
        match &end_result {
            Ok(end) => {
                summary["udp"] = end["udp"].clone();
                emit(end.clone())?;
                eprintln!("{}", serde_json::to_string_pretty(end)?);
            }
            Err(error) => {
                emit(json!({"event":"error","stage":"capture","message":format!("{error:#}")}))?
            }
        }
        if let Err(error) = &tone_result {
            emit(json!({"event":"error","stage":"test_tone","message":format!("{error:#}")}))?;
        }
        emit(summary.clone())?;
        // Histograms can be large; keep them in the JSONL file and show the compact final summary.
        summary
            .as_object_mut()
            .unwrap()
            .remove("callback_histogram_us");
        summary
            .as_object_mut()
            .unwrap()
            .remove("frames_per_callback_histogram");
        println!("{}", serde_json::to_string_pretty(&summary)?);
        end_result?;
        tone_result?;
        Ok(())
    })?;
    if let Some(mut file) = file {
        file.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_gap_is_not_a_large_audio_block() {
        let mut stats = Stats::default();
        stats.record(
            Record::Wake {
                time_100ns: 10_000_000,
                interval_us: Some(500_000),
                frames: 480,
                packets: 1,
                drain_us: 1,
            },
            48000,
        );
        assert_eq!(stats.gaps_ge_500ms, 1);
        assert_eq!(stats.blocks_ge_500ms, 0);
    }
    #[test]
    fn multiple_small_packets_can_form_a_half_second_batch() {
        let mut stats = Stats::default();
        stats.record(
            Record::Wake {
                time_100ns: 10_000_000,
                interval_us: Some(500_000),
                frames: 24000,
                packets: 50,
                drain_us: 10,
            },
            48000,
        );
        assert_eq!(stats.callback_audio_ge_500ms, 1);
        assert_eq!(stats.blocks_ge_500ms, 0);
    }
    #[test]
    fn invalid_or_negative_timestamps_do_not_underflow() {
        let mut stats = Stats::default();
        stats.record(
            Record::Capture {
                sequence: 0,
                wake_time_100ns: None,
                get_buffer_start_100ns: None,
                first_sample: 0,
                device_position: 0,
                device_position_valid: false,
                frames: 480,
                flags: 4,
                capture_time_100ns: 0,
                first_sample_qpc_100ns: None,
                first_sample_age_us: Some(-3),
                get_buffer_us: 0,
                capture_to_record_us: 0,
                peak: None,
            },
            48000,
        );
        assert_eq!(stats.timestamp_errors, 1);
        assert_eq!(stats.negative_ages, 1);
        assert_eq!(stats.age_us.summary().count, 0);
    }
}
