//! Deterministic offline network/clock simulator; memory bounded by in-flight packets.
use receiver_core::{MAX_SAMPLES, Packet, Playout};
use std::{cmp::Ordering, collections::BinaryHeap};
struct Scheduled(Packet);
impl PartialEq for Scheduled {
    fn eq(&self, other: &Self) -> bool {
        self.0.received == other.0.received && self.0.sequence == other.0.sequence
    }
}
impl Eq for Scheduled {}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .received
            .total_cmp(&self.0.received)
            .then_with(|| other.0.sequence.cmp(&self.0.sequence))
    }
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!(
            "--seconds 60 --buffer-ms 20 --delay-ms 0.3 --jitter-ms 0.2 --loss 0.0001 --duplicate 0.0001 --reorder 0.0001 --sender-ppm 12 --receiver-ppm -5 --burst-ms 0 --stall-ms 0 --no-asrc"
        );
        return;
    }
    let val = |key: &str, default: f64| {
        args.iter()
            .position(|a| a == key)
            .map(|i| {
                args.get(i + 1)
                    .expect("missing option value")
                    .parse::<f64>()
                    .expect("invalid numeric option")
            })
            .unwrap_or(default)
    };
    let seconds = val("--seconds", 60.);
    let target = val("--buffer-ms", 20.);
    let delay = val("--delay-ms", 0.3) / 1000.;
    let jitter = val("--jitter-ms", 0.2) / 1000.;
    let loss = val("--loss", 0.0001);
    let duplicate = val("--duplicate", 0.0001);
    let reorder = val("--reorder", 0.0001);
    let sender_ppm = val("--sender-ppm", 12.);
    let receiver_ppm = val("--receiver-ppm", -5.);
    let burst = val("--burst-ms", 0.) / 1000.;
    let stall = val("--stall-ms", 0.) / 1000.;
    assert!(seconds.is_finite() && seconds > 0. && seconds <= 86400.);
    for x in [loss, duplicate, reorder] {
        assert!((0. ..=1.).contains(&x));
    }
    for x in [delay, jitter, burst, stall] {
        assert!(x.is_finite() && (0. ..=1.).contains(&x));
    }
    assert!(sender_ppm.abs() <= 1000. && receiver_ppm.abs() <= 1000.);
    let mut seed = 0x12345678u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut network: BinaryHeap<Scheduled> = BinaryHeap::new();
    let mut seq = 0u64;
    let mut sent = 0;
    let mut dropped = 0;
    let mut block = 0;
    let mut r = Playout::new(48000., target, 0., !args.iter().any(|a| a == "--no-asrc"));
    let mut now = 0.;
    let mut output = [0f32; 256];
    let mut last_stall = 0;
    let mut max_inflight = 0;
    while now < seconds {
        while block as f64 * 0.01 / (1. + sender_ppm * 1e-6) <= now {
            let t = block as f64 * 0.01 / (1. + sender_ppm * 1e-6);
            // Four fragments of each already-available 480-frame Windows block.
            for (offset, frames) in [(0, 128), (128, 128), (256, 128), (384, 96)] {
                let mut p = Packet {
                    stream: 1,
                    session: 1,
                    sequence: seq,
                    first: block * 480 + offset,
                    rate: 48000,
                    frames,
                    channels: 2,
                    flags: 0,
                    acquired: (t * 1e7) as u64,
                    sent: (t * 1e7) as u64,
                    received: 0.,
                    samples: [0.1; MAX_SAMPLES],
                };
                seq += 1;
                sent += 1;
                if random() < loss {
                    dropped += 1;
                    continue;
                }
                let extra = if block % 1000 == 500 { burst } else { 0. };
                p.received = (t
                    + delay
                    + (random() * 2. - 1.) * jitter
                    + extra
                    + if random() < reorder { 0.003 } else { 0. })
                .max(t);
                network.push(Scheduled(p));
                if random() < duplicate {
                    p.received += 0.0001;
                    network.push(Scheduled(p));
                }
            }
            block += 1;
        }
        max_inflight = max_inflight.max(network.len());
        while network.peek().is_some_and(|p| p.0.received <= now) {
            r.receive(&network.pop().unwrap().0);
        }
        r.render(&mut output, now, 1.);
        now += 128. / (48000. * (1. + receiver_ppm * 1e-6));
        let boundary = (now / 10.) as u64;
        if boundary > last_stall {
            now += stall;
            last_stall = boundary;
        }
    }
    println!(
        "{}",
        serde_json::json!({"seconds":seconds,"target_buffer_ms":target,"sender_ppm":sender_ppm,"receiver_ppm":receiver_ppm,"sent":sent,"injected_drops":dropped,"max_inflight_packets":max_inflight,"playout":r.stats})
    );
}
