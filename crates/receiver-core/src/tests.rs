use super::*;
fn packet(first: u64, seq: u64) -> Packet {
    let mut p = Packet {
        stream: 1,
        session: 42,
        sequence: seq,
        first,
        rate: 48000,
        frames: 96,
        channels: 2,
        flags: 0,
        acquired: seq + 1,
        sent: seq + 2,
        received: 0.,
        samples: [0.; MAX_SAMPLES],
    };
    for f in 0..96 {
        p.samples[f * 2] = (first + f as u64 + 1) as f32 / 1000.;
        p.samples[f * 2 + 1] = -p.samples[f * 2];
    }
    p
}
#[test]
fn golden_decode_and_validation() {
    let hex = include_str!("../../../docs/protocol-v1-golden.hex")
        .split_whitespace()
        .collect::<String>();
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let p = Packet::decode(&bytes, 1.).unwrap();
    assert_eq!(
        (
            p.stream, p.session, p.sequence, p.first, p.rate, p.frames, p.channels
        ),
        (1, 3, 4, 5, 48000, 1, 2)
    );
    assert_eq!(&p.samples[..2], &[1., -0.5]);
    assert_eq!((p.acquired, p.sent), (6, 8));
    for i in [0, 4, 6, 13, 14, 15, 46] {
        let mut b = bytes.clone();
        b[i] = 255;
        assert!(Packet::decode(&b, 0.).is_err(), "offset {i}");
    }
    for n in 0..bytes.len() {
        assert!(Packet::decode(&bytes[..n], 0.).is_err());
    }
    let mut b = bytes.clone();
    b[72..76].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(Packet::decode(&b, 0.).is_err());
    let mut b = bytes;
    b[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Packet::decode(&b, 0.).is_err());
}
#[test]
fn reordered_packets_preserve_holes_duplicates_and_deadlines() {
    let mut r = Playout::new(48000., 2., 0., false);
    r.receive(&packet(0, 0));
    r.receive(&packet(192, 2));
    r.receive(&packet(192, 2));
    let mut out = [0.; 576];
    r.render(&mut out, 0.002, 1.);
    assert_eq!(out[0], 0.001);
    assert!(out[192..384].iter().all(|x| *x == 0.));
    assert_eq!(out[384], 0.193);
    r.receive(&packet(96, 1));
    assert_eq!(r.stats.late_packets, 1);
    assert_eq!(r.stats.duplicate_packets, 1);
    assert_eq!(r.stats.reordered_packets, 1);
    assert_eq!(r.stats.missing_frames, 96);
}
#[test]
fn reorder_before_playout_restores_exact_pcm() {
    let mut r = Playout::new(48000., 5., 0., false);
    for (f, s) in [(0, 0), (192, 2), (96, 1)] {
        r.receive(&packet(f, s));
    }
    let mut out = [0.; 576];
    r.render(&mut out, 0.005, 1.);
    for f in 0..288 {
        assert_eq!(out[f * 2], (f + 1) as f32 / 1000.);
    }
    assert_eq!(r.stats.missing_frames, 0);
}
#[test]
fn new_session_discards_old_pcm_and_retired_datagrams() {
    let mut r = Playout::new(48000., 2., 0., false);
    r.receive(&packet(0, 0));
    let mut p = packet(0, 1);
    p.session = 43;
    p.acquired = 100;
    p.samples.fill(0.5);
    r.receive(&p);
    r.receive(&packet(96, 2));
    let mut out = [0.; 192];
    r.render(&mut out, 0.002, 1.);
    assert!(out.iter().all(|x| *x == 0.5));
    assert_eq!(r.stats.sessions, 2);
    assert_eq!(r.stats.rejected_sessions, 1);
}
#[test]
fn av_delay_is_separate_and_sample_accurate() {
    let mut r = Playout::new(48000., 2., 10., false);
    r.receive(&packet(0, 0));
    let mut out = [0.; 192];
    r.render(&mut out, 0.010, 1.);
    assert!(out.iter().all(|x| *x == 0.));
    r.render(&mut out, 0.012, 1.);
    assert_eq!(out[0], 0.001);
}
#[test]
fn large_sample_counters_do_not_lose_precision() {
    let mut r = Playout::new(48000., 2., 0., false);
    let mut p = packet(0, 0);
    p.first = (1u64 << 54) + 13;
    p.samples.fill(0.5);
    r.receive(&p);
    let mut out = [0.; 192];
    r.render(&mut out, 0.002, 1.);
    assert!(out.iter().all(|x| *x == 0.5));
    assert_eq!(r.stats.first_sample, p.first + 96);
}
#[test]
fn source_stop_resume_reanchors_without_replaying_stale_audio() {
    let mut r = Playout::new(48000., 2., 0., false);
    r.receive(&packet(0, 0));
    let mut out = [0.; 192];
    r.render(&mut out, 0.002, 1.);
    let mut p = packet(96, 1);
    p.received = 0.001;
    r.receive(&p);
    for i in 0..600 {
        r.render(&mut out, 0.004 + i as f64 * 0.002, 1.);
    }
    let mut p = packet(192, 2);
    p.received = 1.3;
    r.receive(&p);
    r.render(&mut out, 1.302, 1.);
    assert_eq!(out[0], 0.193);
    assert_eq!(r.stats.rebases, 1);
}
#[test]
fn one_hour_clock_drift_simulator_is_bounded() {
    // 96-frame packets every 2ms, +40ppm source against the output clock.
    let mut r = Playout::new(48000., 20., 0., true);
    let mut next = 0.;
    let mut seq = 0;
    let mut out = [0.; 960];
    for tick in 0..360000 {
        let now = tick as f64 * 0.01;
        while next <= now {
            let mut p = packet(seq * 96, seq);
            p.received = next;
            p.samples.fill(0.1);
            r.receive(&p);
            seq += 1;
            next = seq as f64 * 0.002 / 1.000040;
        }
        r.render(&mut out, now, 1.);
    }
    assert!((r.stats.drift_ppm - 40.).abs() < 1.);
    assert!(r.stats.fill_ms < 35., "{:?}", r.stats);
    assert_eq!(r.stats.missing_frames, 0, "{:?}", r.stats);
    assert!(r.stats.fill_ms > 5.);
}

#[test]
fn callback_stall_discards_elapsed_timeline_instead_of_adding_delay() {
    let mut r = Playout::new(48000., 20., 0., false);
    for seq in 0..40 {
        r.receive(&packet(seq * 96, seq));
    }
    let mut out = [0.; 192];
    r.render(&mut out, 0.02, 1.);
    r.render(&mut out, 0.042, 1.); // 20ms unexpected scheduler stall.
    assert_eq!(r.stats.callback_discontinuities, 1);
    assert_eq!(r.stats.skipped_source_frames, 960);
    assert_eq!(out[0], 1.057); // first sample after 96 rendered + 960 elapsed.
}

#[test]
fn initial_scheduling_delay_does_not_become_permanent_playout_delay() {
    let mut r = Playout::new(48000., 20., 0., false);
    for seq in 0..40 {
        r.receive(&packet(seq * 96, seq));
    }
    let mut out = [0.; 192];
    r.render(&mut out, 0.04, 1.);
    assert_eq!(r.stats.startup_late_frames, 960);
    assert_eq!(out[0], 0.961);
}

#[test]
fn independently_drifting_output_clock_is_compensated() {
    let mut r = Playout::new(48000., 20., 0., true);
    let mut next = 0.;
    let mut seq = 0;
    let mut out = [0.; 960];
    for tick in 0..180000 {
        let now = tick as f64 * 0.01 / 0.999970; // output -30 ppm
        while next <= now {
            let mut p = packet(seq * 96, seq);
            p.received = next;
            p.samples.fill(0.1);
            r.receive(&p);
            seq += 1;
            next = seq as f64 * 0.002 / 1.000020;
        }
        r.render(&mut out, now, 1.);
    }
    assert!((r.stats.sender_clock_ppm - 20.).abs() < 1.);
    assert!((r.stats.receiver_clock_ppm + 30.).abs() < 1.);
    assert!((r.stats.drift_ppm - 50.).abs() < 1.);
    assert!(r.stats.fill_ms < 35.);
    assert_eq!(r.stats.missing_frames, 0);
}
