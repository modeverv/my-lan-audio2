//! Bounded sample-index playout. Only the render owner mutates the timeline.
use serde::Serialize;
pub const MAX_SAMPLES: usize = 332;
pub const CAPACITY: usize = 131072;
#[derive(Clone, Copy, Debug)]
pub struct Packet {
    pub stream: u32,
    pub session: u64,
    pub sequence: u64,
    pub first: u64,
    pub rate: u32,
    pub frames: u16,
    pub channels: u8,
    pub flags: u16,
    pub acquired: u64,
    pub sent: u64,
    pub received: f64,
    pub samples: [f32; MAX_SAMPLES],
}
impl Packet {
    pub fn decode(d: &[u8], received: f64) -> Result<Self, &'static str> {
        if d.len() < 72 || d.len() > 1400 {
            return Err("datagram length");
        }
        let u16at = |i| u16::from_le_bytes(d[i..i + 2].try_into().unwrap());
        let u32at = |i| u32::from_le_bytes(d[i..i + 4].try_into().unwrap());
        let u64at = |i| u64::from_le_bytes(d[i..i + 8].try_into().unwrap());
        if &d[..4] != b"LNAU" || u16at(4) != 1 || u16at(6) != 72 {
            return Err("protocol");
        }
        let (channels, frames, rate, flags) = (d[14], u16at(44), u32at(40), u16at(12));
        if !(1..=8).contains(&channels)
            || frames == 0
            || d[15] != 1
            || !(8000..=192000).contains(&rate)
            || flags & !31 != 0
            || u16at(46) != 0
        {
            return Err("format or reserved fields");
        }
        let count = channels as usize * frames as usize;
        if count > MAX_SAMPLES
            || d.len() != 72 + count * 4
            || u64at(32).checked_add(frames as u64).is_none()
        {
            return Err("payload");
        }
        let mut p = Self {
            stream: u32at(8),
            session: u64at(16),
            sequence: u64at(24),
            first: u64at(32),
            rate,
            frames,
            channels,
            flags,
            acquired: u64at(48),
            sent: u64at(64),
            received,
            samples: [0.; MAX_SAMPLES],
        };
        for (out, b) in p.samples[..count].iter_mut().zip(d[72..].chunks_exact(4)) {
            let value = f32::from_le_bytes(b.try_into().unwrap());
            if !value.is_finite() {
                return Err("non-finite PCM");
            }
            *out = if flags & 1 != 0 { 0. } else { value };
        }
        Ok(p)
    }
}
#[derive(Default, Clone, Copy, Serialize, Debug)]
pub struct Stats {
    pub packets: u64,
    pub sessions: u64,
    pub late_packets: u64,
    pub duplicate_packets: u64,
    pub reordered_packets: u64,
    pub rejected_sessions: u64,
    pub missing_frames: u64,
    pub underruns: u64,
    pub rebases: u64,
    pub overflow_packets: u64,
    pub output_frames: u64,
    pub rendered_signal_frames: u64,
    pub session_id: u64,
    pub first_sample: u64,
    pub fill_ms: f64,
    pub min_fill_ms: f64,
    pub max_fill_ms: f64,
    pub drift_ppm: f64,
    pub sender_clock_ppm: f64,
    pub receiver_clock_ppm: f64,
    pub asrc_ratio: f64,
    pub peak: f32,
    pub fill_observations: u64,
    pub callback_discontinuities: u64,
    pub startup_late_frames: u64,
    pub skipped_source_frames: u64,
    pub render_first_sample: u64,
    pub playout_sequence: u64,
    pub receive_to_render_min_ms: f64,
    pub receive_to_render_max_ms: f64,
    pub receive_to_render_sum_ms: f64,
    pub received_frame_observations: u64,
}
#[derive(Clone, Copy)]
struct Slot {
    generation: u64,
    sample: u64,
    value: [f32; 2],
    sequence: u64,
    received: f64,
}
/// Regression on local receive time; timestamps from Windows are never subtracted from Mac time.
#[derive(Default)]
struct Drift {
    origin: Option<(f64, u64)>,
    n: f64,
    x: f64,
    y: f64,
    xx: f64,
    xy: f64,
    estimate: f64,
}
impl Drift {
    fn add(&mut self, now: f64, sample: u64, rate: f64) {
        let (t, s) = *self.origin.get_or_insert((now, sample));
        if sample < s {
            return;
        }
        let x = now - t;
        let y = (sample - s) as f64 / rate;
        self.n += 1.;
        self.x += x;
        self.y += y;
        self.xx += x * x;
        self.xy += x * y;
        if x >= 10. {
            let den = self.n * self.xx - self.x * self.x;
            if den > 0. {
                let ppm = ((self.n * self.xy - self.x * self.y) / den - 1.) * 1e6;
                if ppm.abs() < 3000. {
                    self.estimate += 0.2 * (ppm.clamp(-1000., 1000.) - self.estimate);
                }
            }
            let e = self.estimate;
            *self = Self::default();
            self.estimate = e;
        }
    }
}
pub struct Playout {
    slots: Vec<Slot>,
    seen: Vec<Option<u64>>,
    generation: u64,
    session: Option<u64>,
    max_acquired: u64,
    max_sequence: Option<u64>,
    cursor: u64,
    fraction: f64,
    start_time: f64,
    newest: u64,
    rate: f64,
    last_received: f64,
    active: bool,
    last_render: Option<(f64, usize)>,
    output_rate: f64,
    delay: f64,
    drift: Drift,
    receiver_drift: Drift,
    hardware_frames: u64,
    correction: f64,
    asrc: bool,
    pub stats: Stats,
}
impl Playout {
    pub fn new(output_rate: f64, buffer_ms: f64, av_ms: f64, asrc: bool) -> Self {
        assert!((8000. ..=192000.).contains(&output_rate));
        assert!(buffer_ms.is_finite() && (2. ..=100.).contains(&buffer_ms));
        assert!(av_ms.is_finite() && (0. ..=250.).contains(&av_ms));
        Self {
            slots: vec![
                Slot {
                    generation: 0,
                    sample: 0,
                    value: [0.; 2],
                    sequence: 0,
                    received: 0.,
                };
                CAPACITY
            ],
            seen: vec![None; 8192],
            generation: 0,
            session: None,
            max_acquired: 0,
            max_sequence: None,
            cursor: 0,
            fraction: 0.,
            start_time: 0.,
            newest: 0,
            rate: 48000.,
            last_received: 0.,
            active: false,
            last_render: None,
            output_rate,
            delay: (buffer_ms + av_ms) / 1000.,
            drift: Drift::default(),
            receiver_drift: Drift::default(),
            hardware_frames: 0,
            correction: 0.,
            asrc,
            stats: Stats::default(),
        }
    }
    fn reset(&mut self, p: &Packet, new_session: bool) {
        self.generation += 1;
        self.session = Some(p.session);
        self.cursor = p.first;
        self.fraction = 0.;
        self.rate = p.rate as f64;
        self.start_time = p.received + self.delay;
        self.newest = p.first;
        self.active = false;
        self.last_render = None;
        self.drift = Drift::default();
        self.correction = 0.;
        if new_session {
            self.seen.fill(None);
            self.max_sequence = None;
            self.stats.sessions += 1;
        }
        self.stats.session_id = p.session;
    }
    pub fn receive(&mut self, p: &Packet) {
        // This stereo renderer intentionally rejects multichannel input rather than silently remixing it.
        if p.channels > 2 {
            return;
        }
        if self.session != Some(p.session) {
            if self.session.is_some() && p.acquired < self.max_acquired {
                self.stats.rejected_sessions += 1;
                return;
            }
            self.reset(p, true);
        } else if p.rate as f64 != self.rate {
            return;
        }
        self.max_acquired = self.max_acquired.max(p.acquired);
        let si = p.sequence as usize % self.seen.len();
        if self.seen[si] == Some(p.sequence) {
            self.stats.duplicate_packets += 1;
            return;
        }
        self.seen[si] = Some(p.sequence);
        if self.max_sequence.is_some_and(|s| p.sequence < s) {
            self.stats.reordered_packets += 1;
        }
        self.max_sequence = Some(self.max_sequence.map_or(p.sequence, |s| s.max(p.sequence)));
        self.stats.packets += 1;
        if p.received - self.last_received > 0.5 && self.last_received > 0. {
            self.reset(p, false);
            self.stats.rebases += 1;
        }
        self.last_received = p.received;
        let end = p.first + p.frames as u64;
        if !self.active && p.first < self.cursor {
            self.start_time -= (self.cursor - p.first) as f64 / self.rate;
            self.cursor = p.first;
        }
        if end <= self.cursor {
            self.stats.late_packets += 1;
            return;
        }
        if p.first.saturating_sub(self.cursor) >= CAPACITY as u64 {
            self.stats.overflow_packets += 1;
            return;
        }
        if p.first < self.cursor {
            self.stats.late_packets += 1;
        }
        if end > self.newest {
            self.newest = end;
            self.drift.add(p.received, end, self.rate);
        }
        for f in 0..p.frames as usize {
            let sample = p.first + f as u64;
            if sample < self.cursor || sample - self.cursor >= CAPACITY as u64 {
                continue;
            }
            let ch = p.channels as usize;
            self.slots[sample as usize % CAPACITY] = Slot {
                generation: self.generation,
                sample,
                value: [p.samples[f * ch], p.samples[f * ch + ch - 1]],
                sequence: p.sequence,
                received: p.received,
            };
        }
    }
    fn sample(&self, s: u64) -> Option<[f32; 2]> {
        let slot = &self.slots[s as usize % CAPACITY];
        (slot.generation == self.generation && slot.sample == s).then_some(slot.value)
    }
    pub fn render(&mut self, out: &mut [f32], now: f64, gain: f32) {
        out.fill(0.);
        if self.session.is_none() {
            return;
        }
        // An inactive sender is shown as waiting, rather than counting hours of silence as loss.
        if now - self.last_received > 0.5 && self.cursor >= self.newest {
            return;
        }
        let frames = out.len() / 2;
        let target = if self.asrc {
            (self.drift.estimate - self.receiver_drift.estimate).clamp(-1000., 1000.)
        } else {
            0.
        };
        let step = 10. * frames as f64 / self.output_rate;
        self.correction += (target - self.correction).clamp(-step, step);
        let ratio = self.rate / self.output_rate * (1. + self.correction * 1e-6);
        // A stalled callback must not turn into a permanent extra audio queue.
        if self.active
            && let Some((last, previous_frames)) = self.last_render
        {
            let expected = previous_frames as f64 / self.output_rate;
            let gap = now - last - expected;
            if gap > expected.max(0.005) {
                let skipped = (gap * self.rate * (1. + self.correction * 1e-6)).round() as u64;
                self.cursor = self.cursor.saturating_add(skipped);
                self.stats.callback_discontinuities += 1;
                self.receiver_drift = Drift::default();
                self.stats.skipped_source_frames += skipped;
            }
        }
        self.receiver_drift
            .add(now, self.hardware_frames, self.output_rate);
        self.hardware_frames += frames as u64;
        self.last_render = Some((now, frames));
        self.stats.render_first_sample = self.cursor;
        let mut missing = false;
        for (i, o) in out.chunks_exact_mut(2).enumerate() {
            if !self.active {
                if now + i as f64 / self.output_rate < self.start_time {
                    continue;
                }
                let overdue = (now + i as f64 / self.output_rate - self.start_time).max(0.);
                let skip = (overdue * self.rate).floor() as u64;
                self.cursor = self.cursor.saturating_add(skip);
                self.stats.startup_late_frames += skip;
                self.stats.render_first_sample = self.cursor;
                self.active = true;
            }
            let a = self.sample(self.cursor);
            // Linear ASRC is a replaceable, zero-lookahead-delay baseline. A missing next frame
            // holds only this fractional interval; missing current frames always remain zero.
            if let Some(a) = a {
                let slot = self.slots[self.cursor as usize % CAPACITY];
                self.stats.playout_sequence = slot.sequence;
                let wait_ms = (now + i as f64 / self.output_rate - slot.received) * 1000.;
                if self.stats.received_frame_observations == 0 {
                    self.stats.receive_to_render_min_ms = wait_ms;
                }
                self.stats.receive_to_render_min_ms =
                    self.stats.receive_to_render_min_ms.min(wait_ms);
                self.stats.receive_to_render_max_ms =
                    self.stats.receive_to_render_max_ms.max(wait_ms);
                self.stats.receive_to_render_sum_ms += wait_ms;
                self.stats.received_frame_observations += 1;
                let b = self.sample(self.cursor + 1).unwrap_or(a);
                for ch in 0..2 {
                    o[ch] = (a[ch] + (b[ch] - a[ch]) * self.fraction as f32) * gain;
                }
                let peak = o[0].abs().max(o[1].abs());
                self.stats.peak = self.stats.peak.max(peak);
                if peak > 1e-7 {
                    self.stats.rendered_signal_frames += 1;
                }
            } else {
                self.stats.missing_frames += 1;
                missing = true;
            }
            self.fraction += ratio;
            let advance = self.fraction.floor() as u64;
            self.cursor = self.cursor.saturating_add(advance);
            self.fraction -= advance as f64;
            self.stats.output_frames += 1;
        }
        if missing {
            self.stats.underruns += 1;
        }
        let fill = self.newest.saturating_sub(self.cursor) as f64 / self.rate * 1000.;
        self.stats.fill_ms = fill;
        if self.active {
            if self.stats.fill_observations == 0 {
                self.stats.min_fill_ms = fill;
                self.stats.max_fill_ms = fill;
            } else {
                self.stats.min_fill_ms = self.stats.min_fill_ms.min(fill);
                self.stats.max_fill_ms = self.stats.max_fill_ms.max(fill);
            }
            self.stats.fill_observations += 1;
        }
        self.stats.sender_clock_ppm = self.drift.estimate;
        self.stats.receiver_clock_ppm = self.receiver_drift.estimate;
        self.stats.drift_ppm = self.drift.estimate - self.receiver_drift.estimate;
        self.stats.asrc_ratio = ratio;
        self.stats.first_sample = self.cursor;
    }
}

#[cfg(test)]
mod tests;
