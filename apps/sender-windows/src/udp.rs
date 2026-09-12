//! Bounded preallocated publication; only workers touch nonblocking UDP sockets.
use anyhow::{Context, Result, ensure};
use crossbeam_queue::ArrayQueue;
use serde_json::{Value, json};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

pub const HEADER: usize = 72;
pub const MAX_DATAGRAM: usize = 1400;
const SLOTS: usize = 64;

#[derive(Clone, Copy)]
pub struct Clock(i64);
impl Clock {
    pub fn new() -> Result<Self> {
        let mut frequency = 0;
        unsafe {
            QueryPerformanceFrequency(&mut frequency)?;
        }
        Ok(Self(frequency))
    }
    pub fn now(self) -> u64 {
        let mut value = 0;
        unsafe {
            QueryPerformanceCounter(&mut value).expect("QPC unavailable");
        }
        (value as u128 * 10_000_000 / self.0 as u128) as u64
    }
}

struct Packet {
    bytes: [u8; MAX_DATAGRAM],
    len: usize,
    acquired: u64,
    published: u64,
}
struct Queues {
    free: ArrayQueue<Box<Packet>>,
    ready: ArrayQueue<Box<Packet>>,
    stop: AtomicBool,
}
impl Queues {
    fn new() -> Self {
        let free = ArrayQueue::new(SLOTS);
        for _ in 0..SLOTS {
            let _ = free.push(Box::new(Packet {
                bytes: [0; MAX_DATAGRAM],
                len: 0,
                acquired: 0,
                published: 0,
            }));
        }
        Self {
            free,
            ready: ArrayQueue::new(SLOTS),
            stop: AtomicBool::new(false),
        }
    }
    fn recycle(&self, packet: Box<Packet>) {
        // One fixed pool shared by producer and workers; capacity cannot be exceeded.
        assert!(self.free.push(packet).is_ok());
    }
}

#[derive(Clone, Copy)]
struct Header {
    stream: u32,
    flags: u16,
    channels: u8,
    session: u64,
    sequence: u64,
    first_sample: u64,
    rate: u32,
    frames: u16,
    acquired: u64,
    published: u64,
}
fn encode(out: &mut [u8], h: Header) {
    out[..HEADER].fill(0);
    out[..4].copy_from_slice(b"LNAU");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6..8].copy_from_slice(&(HEADER as u16).to_le_bytes());
    out[8..12].copy_from_slice(&h.stream.to_le_bytes());
    out[12..14].copy_from_slice(&h.flags.to_le_bytes());
    out[14] = h.channels;
    out[15] = 1; // interleaved f32 little endian
    out[16..24].copy_from_slice(&h.session.to_le_bytes());
    out[24..32].copy_from_slice(&h.sequence.to_le_bytes());
    out[32..40].copy_from_slice(&h.first_sample.to_le_bytes());
    out[40..44].copy_from_slice(&h.rate.to_le_bytes());
    out[44..46].copy_from_slice(&h.frames.to_le_bytes());
    out[48..56].copy_from_slice(&h.acquired.to_le_bytes());
    out[56..64].copy_from_slice(&h.published.to_le_bytes());
}

// Preallocated microsecond histogram. Last bucket means >=10ms; retain exact max.
struct Timing {
    bins: Vec<u64>,
    count: u64,
    min: u64,
    max: u64,
}
impl Timing {
    fn new() -> Self {
        Self {
            bins: vec![0; 10001],
            count: 0,
            min: u64::MAX,
            max: 0,
        }
    }
    fn record(&mut self, ticks: u64) {
        let us = ticks / 10;
        self.bins[us.min(10000) as usize] += 1;
        self.count += 1;
        self.min = self.min.min(us);
        self.max = self.max.max(us);
    }
    fn summary(&self) -> Value {
        let percentile = |p: u64| {
            let rank = (self.count * p).div_ceil(100);
            let mut n = 0;
            self.bins
                .iter()
                .position(|v| {
                    n += v;
                    n >= rank
                })
                .filter(|_| self.count != 0)
        };
        json!({"count":self.count,"min":(self.count!=0).then_some(self.min),"max":(self.count!=0).then_some(self.max),
            "p50":percentile(50),"p99":percentile(99),"overflow_ge_10000us":self.bins[10000]})
    }
}

struct WorkerStats {
    mmcss_error: Option<String>,
    sent: u64,
    bytes: u64,
    stale: u64,
    errors: u64,
    would_block: u64,
    last_error: Option<i32>,
    acquire_to_send: Timing,
    publish_to_send: Timing,
    send_call: Timing,
}
impl WorkerStats {
    fn new() -> Self {
        Self {
            mmcss_error: None,
            sent: 0,
            bytes: 0,
            stale: 0,
            errors: 0,
            would_block: 0,
            last_error: None,
            acquire_to_send: Timing::new(),
            publish_to_send: Timing::new(),
            send_call: Timing::new(),
        }
    }
    fn summary(self) -> Value {
        json!({"mmcss_error":self.mmcss_error,"sent":self.sent,"bytes":self.bytes,"deadline_drops":self.stale,"send_errors":self.errors,
            "would_block_drops":self.would_block,"last_os_error":self.last_error,
            "acquire_to_send_us":self.acquire_to_send.summary(),"publish_to_send_us":self.publish_to_send.summary(),
            "send_call_us":self.send_call.summary()})
    }
}
fn worker(q: Arc<Queues>, socket: UdpSocket, clock: Clock, deadline: u64) -> WorkerStats {
    use windows::Win32::System::Threading::{
        AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
    };
    struct Mmcss(windows::Win32::Foundation::HANDLE);
    impl Drop for Mmcss {
        fn drop(&mut self) {
            unsafe {
                let _ = AvRevertMmThreadCharacteristics(self.0);
            }
        }
    }
    let mut task = 0;
    let registration =
        unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task) };
    let mut stats = WorkerStats::new();
    stats.mmcss_error = registration.as_ref().err().map(ToString::to_string);
    let _mmcss = registration.ok().map(Mmcss);
    loop {
        if let Some(mut packet) = q.ready.pop() {
            let now = clock.now();
            if now.saturating_sub(packet.acquired) > deadline {
                stats.stale += 1;
            } else {
                packet.bytes[64..72].copy_from_slice(&now.to_le_bytes());
                match socket.send(&packet.bytes[..packet.len]) {
                    Ok(n) if n == packet.len => {
                        stats.sent += 1;
                        stats.bytes += n as u64;
                        stats
                            .acquire_to_send
                            .record(now.saturating_sub(packet.acquired));
                        stats
                            .publish_to_send
                            .record(now.saturating_sub(packet.published));
                    }
                    Ok(_) => {
                        stats.errors += 1;
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            stats.would_block += 1;
                        } else {
                            stats.errors += 1;
                        }
                        stats.last_error = e.raw_os_error();
                    }
                }
                stats.send_call.record(clock.now().saturating_sub(now));
            }
            q.recycle(packet);
        } else if q.stop.load(Ordering::Acquire) {
            break;
        } else {
            thread::park();
        }
    }
    stats
}

pub struct Sender {
    q: Arc<Queues>,
    workers: Vec<JoinHandle<WorkerStats>>,
    clock: Clock,
    channels: u8,
    rate: u32,
    packet_frames: u16,
    stream: u32,
    session: u64,
    sequence: u64,
    sample: u64,
    initialized: bool,
    previous_position_end: Option<u64>,
    published: u64,
    pool_drops: u64,
    restarts: u64,
    acquire_to_publish: Timing,
}
impl Sender {
    pub fn new(
        destination: SocketAddr,
        count: u8,
        deadline_ms: u32,
        packet_frames: u16,
        stream: u32,
        rate: u32,
        channels: u16,
    ) -> Result<Self> {
        ensure!((1..=3).contains(&count), "worker count must be 1..3");
        ensure!(
            (1..=8).contains(&channels) && rate != 0 && packet_frames != 0,
            "unsupported PCM format"
        );
        ensure!(
            HEADER + packet_frames as usize * channels as usize * 4 <= MAX_DATAGRAM,
            "UDP frame limit exceeds 1400 byte datagram cap"
        );
        let socket = UdpSocket::bind(if destination.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        socket
            .connect(destination)
            .context("connect UDP destination")?;
        socket.set_nonblocking(true)?;
        let sockets = (0..count)
            .map(|_| socket.try_clone())
            .collect::<std::io::Result<Vec<_>>>()?;
        let clock = Clock::new()?;
        let q = Arc::new(Queues::new());
        let session = unsafe { windows::Win32::System::Com::CoCreateGuid()? }.to_u128() as u64;
        let mut sender = Self {
            q,
            workers: Vec::new(),
            clock,
            channels: channels as u8,
            rate,
            packet_frames,
            stream,
            session,
            sequence: 0,
            sample: 0,
            initialized: false,
            previous_position_end: None,
            published: 0,
            pool_drops: 0,
            restarts: 0,
            acquire_to_publish: Timing::new(),
        };
        for socket in sockets {
            let q = sender.q.clone();
            sender.workers.push(
                thread::Builder::new()
                    .name("udp-send".into())
                    .spawn(move || worker(q, socket, clock, deadline_ms as u64 * 10000))?,
            );
        }
        Ok(sender)
    }

    /// Bytes must remain borrowed only until return. None is a WASAPI silent buffer.
    pub fn publish(
        &mut self,
        bytes: Option<&[u8]>,
        frames: u32,
        flags: u32,
        acquired: u64,
        position: Option<u64>,
    ) {
        let broken = flags & (1 | 4) != 0
            || position
                .zip(self.previous_position_end)
                .is_some_and(|(p, e)| p != e);
        if self.initialized && broken {
            self.session = self.session.wrapping_add(1);
            self.sequence = 0;
            self.sample = 0;
            self.restarts += 1;
        }
        self.initialized = true;
        self.previous_position_end = position.map(|p| p + frames as u64);
        let align = self.channels as usize * 4;
        for offset in (0..frames).step_by(self.packet_frames as usize) {
            let n = (frames - offset).min(self.packet_frames as u32) as u16;
            if let Some(mut packet) = self.q.free.pop() {
                let len = n as usize * align;
                packet.len = HEADER + len;
                packet.acquired = acquired;
                if let Some(bytes) = bytes {
                    packet.bytes[HEADER..packet.len].copy_from_slice(
                        &bytes[offset as usize * align..offset as usize * align + len],
                    );
                } else {
                    packet.bytes[HEADER..packet.len].fill(0);
                }
                let wire_flags = u16::from(bytes.is_none())
                    | (u16::from(self.sequence == 0) << 1)
                    | (u16::from(flags & 4 != 0) << 2)
                    | (u16::from(flags & 1 != 0) << 3)
                    | (u16::from(position.is_none()) << 4);
                packet.published = self.clock.now();
                encode(
                    &mut packet.bytes,
                    Header {
                        stream: self.stream,
                        flags: wire_flags,
                        channels: self.channels,
                        session: self.session,
                        sequence: self.sequence,
                        first_sample: self.sample + offset as u64,
                        rate: self.rate,
                        frames: n,
                        acquired,
                        published: packet.published,
                    },
                );
                self.acquire_to_publish
                    .record(packet.published.saturating_sub(acquired));
                if let Err(packet) = self.q.ready.push(packet) {
                    self.pool_drops += 1;
                    self.q.recycle(packet);
                } else {
                    self.published += 1;
                    for worker in &self.workers {
                        worker.thread().unpark();
                    }
                }
            } else {
                self.pool_drops += 1;
            }
            // Dropped audio retains its sequence and sample-index hole.
            self.sequence += 1;
        }
        self.sample += frames as u64;
    }
    pub fn finish(&mut self) -> Value {
        self.q.stop.store(true, Ordering::Release);
        for w in &self.workers {
            w.thread().unpark();
        }
        let workers: Vec<_> = self
            .workers
            .drain(..)
            .map(|w| match w.join() {
                Ok(stats) => stats.summary(),
                Err(_) => json!({"worker_panicked":true}),
            })
            .collect();
        json!({"published_packets":self.published,"pool_drops":self.pool_drops,"session_restarts":self.restarts,
            "pool_slots":SLOTS,"acquire_to_publish_us":self.acquire_to_publish.summary(),"workers":workers})
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            self.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn idle_sender() -> Sender {
        let destination = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut sender =
            Sender::new(destination.local_addr().unwrap(), 1, 5, 128, 1, 48000, 2).unwrap();
        sender.finish(); // Deterministic publication-only tests with no consuming worker.
        sender
    }
    fn number(bytes: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }
    #[test]
    fn splits_existing_block_without_padding_or_waiting_and_copies_exact_pcm() {
        let mut sender = idle_sender();
        let pcm: Vec<u8> = (0..480 * 8).map(|i| (i % 251) as u8).collect();
        sender.publish(Some(&pcm), 480, 0, sender.clock.now(), None);
        let mut restored = Vec::new();
        let mut sizes = Vec::new();
        let mut sample = 0;
        while let Some(packet) = sender.q.ready.pop() {
            assert!(packet.len <= MAX_DATAGRAM);
            assert_eq!(number(&packet.bytes, 32), sample);
            let frames = u16::from_le_bytes(packet.bytes[44..46].try_into().unwrap());
            sizes.push(frames);
            sample += frames as u64;
            restored.extend_from_slice(&packet.bytes[HEADER..packet.len]);
            sender.q.recycle(packet);
        }
        assert_eq!(sizes, [128, 128, 128, 96]);
        assert_eq!(restored, pcm);
        sender.publish(None, 1, 2, sender.clock.now(), None);
        let packet = sender.q.ready.pop().unwrap();
        assert_eq!(packet.len, HEADER + 8);
        assert_eq!(&packet.bytes[HEADER..packet.len], &[0; 8]);
        assert_ne!(packet.bytes[12] & 1, 0);
    }
    #[test]
    fn exhausted_pool_preserves_holes_and_discontinuity_restarts_session() {
        let mut sender = idle_sender();
        sender.publish(None, 128 * 65, 0, sender.clock.now(), None);
        assert_eq!(sender.pool_drops, 1);
        while let Some(packet) = sender.q.ready.pop() {
            sender.q.recycle(packet);
        }
        sender.publish(None, 128, 0, sender.clock.now(), None);
        let packet = sender.q.ready.pop().unwrap();
        assert_eq!(number(&packet.bytes, 24), 65);
        assert_eq!(number(&packet.bytes, 32), 128 * 65);
        let old_session = number(&packet.bytes, 16);
        sender.q.recycle(packet);
        sender.publish(None, 128, 1, sender.clock.now(), None);
        let packet = sender.q.ready.pop().unwrap();
        assert_ne!(number(&packet.bytes, 16), old_session);
        assert_eq!(number(&packet.bytes, 24), 0);
        assert_eq!(number(&packet.bytes, 32), 0);
        assert_eq!(sender.restarts, 1);
    }
    #[test]
    fn device_position_gap_starts_new_timeline() {
        let mut sender = idle_sender();
        sender.publish(None, 128, 0, sender.clock.now(), Some(500));
        sender.publish(None, 128, 0, sender.clock.now(), Some(800));
        assert_eq!(sender.restarts, 1);
    }
    #[test]
    fn expired_audio_is_dropped_without_sending() {
        let destination = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut sender =
            Sender::new(destination.local_addr().unwrap(), 1, 1, 128, 1, 48000, 2).unwrap();
        sender.publish(None, 480, 0, 0, None);
        let result = sender.finish();
        assert_eq!(result["workers"][0]["sent"], 0);
        assert_eq!(result["workers"][0]["deadline_drops"], 4);
    }
    #[test]
    fn golden_header_has_explicit_little_endian_layout() {
        let mut bytes = [0xff; HEADER];
        encode(
            &mut bytes,
            Header {
                stream: 1,
                flags: 0x12,
                channels: 2,
                session: 3,
                sequence: 4,
                first_sample: 5,
                rate: 48000,
                frames: 128,
                acquired: 6,
                published: 7,
            },
        );
        let expected = [
            76, 78, 65, 85, 1, 0, 72, 0, 1, 0, 0, 0, 18, 0, 2, 1, 3, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0,
            0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 128, 187, 0, 0, 128, 0, 0, 0, 6, 0, 0, 0, 0, 0,
            0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(bytes, expected);
    }
    #[test]
    fn shared_golden_datagram_includes_pcm_and_send_timestamp() {
        let expected: Vec<u8> = include_str!("../../../docs/protocol-v1-golden.hex")
            .split_whitespace()
            .flat_map(|line| {
                line.as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            })
            .collect();
        let mut bytes = [0; HEADER + 8];
        encode(
            &mut bytes,
            Header {
                stream: 1,
                flags: 16,
                channels: 2,
                session: 3,
                sequence: 4,
                first_sample: 5,
                rate: 48000,
                frames: 1,
                acquired: 6,
                published: 7,
            },
        );
        bytes[64..72].copy_from_slice(&8u64.to_le_bytes());
        bytes[72..76].copy_from_slice(&1f32.to_le_bytes());
        bytes[76..80].copy_from_slice(&(-0.5f32).to_le_bytes());
        assert_eq!(bytes.as_slice(), expected);
    }
}
