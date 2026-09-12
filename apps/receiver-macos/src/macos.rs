use anyhow::{Context, bail, ensure};
use clap::{Parser, ValueEnum};
use crossbeam_queue::ArrayQueue;
use receiver_core::{Packet, Playout, Stats};
use serde_json::{Value, json};
use std::{
    ffi::{CStr, c_char, c_void},
    io::Write,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    os::fd::AsRawFd,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, ValueEnum)]
enum NetworkScheduling {
    Qos,
    Realtime,
}
#[derive(Parser, Debug)]
#[command(about = "LNAU v1 macOS receiver / CoreAudio output")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:40100")]
    bind: SocketAddr,
    #[arg(long)]
    source: Option<IpAddr>,
    /// IPv4 multicast group to join (for example 239.255.0.1).
    #[arg(long)]
    multicast_group: Option<Ipv4Addr>,
    /// Local Mac IPv4 address for group membership; 0.0.0.0 lets the OS choose.
    #[arg(long, default_value = "0.0.0.0", requires = "multicast_group")]
    multicast_interface: Ipv4Addr,
    #[arg(long, default_value_t = 1)]
    stream: u32,
    #[arg(long)]
    list_devices: bool,
    #[arg(long, default_value = "Fireface UCX II")]
    device: String,
    /// One-based first output channel. Stereo uses this and the next channel.
    #[arg(long, default_value_t = 1)]
    channel: u32,
    /// Zero keeps the device's existing buffer size. Explicit changes are restored on stop.
    #[arg(long, default_value_t = 0)]
    buffer_frames: u32,
    #[arg(long, default_value_t = 20.)]
    buffer_ms: f64,
    #[arg(long, default_value_t = 0.)]
    av_sync_delay_ms: f64,
    #[arg(long,default_value_t=-12.,allow_hyphen_values=true)]
    gain_db: f32,
    #[arg(long)]
    no_asrc: bool,
    #[arg(long)]
    diagnostic: bool,
    /// Zero runs until Stop/Ctrl+C.
    #[arg(long, default_value_t = 0.)]
    seconds: f64,
    #[arg(long)]
    output: Option<std::path::PathBuf>,
    /// Include packet metadata (never PCM) for joining sender logs.
    #[arg(long)]
    events: bool,
    /// Deadline scheduling for the dedicated socket thread, or original interactive QoS.
    #[arg(long,value_enum,default_value_t=NetworkScheduling::Realtime)]
    network_scheduling: NetworkScheduling,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Device {
    id: u32,
    channels: u32,
    buffer_frames: u32,
    latency_frames: u32,
    safety_frames: u32,
    rate: f64,
    name: [c_char; 256],
}
impl Device {
    fn name(&self) -> String {
        unsafe { CStr::from_ptr(self.name.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
    fn json(&self) -> Value {
        json!({"id":self.id,"name":self.name(),"channels":self.channels,"sample_rate":self.rate,"buffer_frames":self.buffer_frames,"buffer_ms":self.buffer_frames as f64/self.rate*1000.,"device_latency_frames":self.latency_frames,"safety_offset_frames":self.safety_frames})
    }
}
#[repr(C)]
#[derive(Default)]
struct Receipt {
    address: [u8; 16],
    port: u16,
    ipv6: u8,
    kernel_queue_ms: f64,
    interface_index: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Scheduling {
    qos_status: i32,
    set_status: i32,
    get_status: i32,
    is_default: i32,
    period: u32,
    computation: u32,
    constraint: u32,
    numer: u32,
    denom: u32,
}
#[derive(Clone, Copy)]
struct QueuedPacket {
    packet: Packet,
    queued_at: f64,
}
unsafe extern "C" {
    fn lan_prepare_socket(fd: i32) -> i32;
    fn lan_receive(fd: i32, bytes: *mut u8, capacity: usize, receipt: *mut Receipt) -> isize;
    fn lan_devices(out: *mut Device, capacity: i32) -> i32;
    fn lan_start(
        device: *mut Device,
        channel: u32,
        frames: u32,
        render: extern "C" fn(*mut c_void, *mut f32, u32, f64),
        context: *mut c_void,
        error: *mut i32,
    ) -> *mut c_void;
    fn lan_stop(output: *mut c_void);
    fn lan_network_priority(realtime: i32, report: *mut Scheduling);
}
struct Output(*mut c_void);
impl Drop for Output {
    fn drop(&mut self) {
        unsafe { lan_stop(self.0) }
    }
}
#[derive(Clone, Copy)]
struct Snapshot {
    stats: Stats,
    interval_ms: f64,
    work_ms: f64,
    presentation_lead_ms: f64,
    handoff_max_ms: f64,
    handoff_packets: usize,
    frames: u32,
    time: f64,
}
struct RenderState {
    playout: Playout,
    packets: Arc<ArrayQueue<QueuedPacket>>,
    snapshots: Arc<ArrayQueue<Snapshot>>,
    epoch: Instant,
    last: f64,
    gain: f32,
    telemetry_dropped: Arc<AtomicU64>,
    last_callback_ns: Arc<AtomicU64>,
}
extern "C" fn render(ctx: *mut c_void, samples: *mut f32, frames: u32, presentation_lead_ms: f64) {
    // AUHAL owns the callback serially; RenderState stays boxed until lan_stop has returned.
    let state = unsafe { &mut *(ctx as *mut RenderState) };
    let now = state.epoch.elapsed().as_secs_f64();
    state
        .last_callback_ns
        .store((now * 1e9) as u64, Ordering::Relaxed);
    // Bound work even if the producer keeps filling the queue during this callback.
    let available = state.packets.len().min(256);
    let mut handoff_max_ms = 0f64;
    let mut handoff_packets = 0;
    for _ in 0..available {
        if let Some(p) = state.packets.pop() {
            handoff_max_ms =
                handoff_max_ms.max((state.epoch.elapsed().as_secs_f64() - p.queued_at) * 1000.);
            handoff_packets += 1;
            state.playout.receive(&p.packet);
        }
    }
    let out = unsafe { std::slice::from_raw_parts_mut(samples, frames as usize * 2) };
    state.playout.render(out, now, state.gain);
    let snapshot = Snapshot {
        handoff_max_ms,
        handoff_packets,
        presentation_lead_ms,
        stats: state.playout.stats,
        interval_ms: if state.last > 0. {
            (now - state.last) * 1000.
        } else {
            0.
        },
        work_ms: (state.epoch.elapsed().as_secs_f64() - now) * 1000.,
        frames,
        time: now,
    };
    if state.snapshots.push(snapshot).is_err() {
        state.telemetry_dropped.fetch_add(1, Ordering::Relaxed);
    }
    state.last = now;
}
#[derive(Default)]
struct NetworkStats {
    received: AtomicU64,
    invalid: AtomicU64,
    foreign: AtomicU64,
    queue_dropped: AtomicU64,
    retired: AtomicU64,
}
#[derive(Clone, Copy)]
struct NetEvent {
    session: u64,
    sequence: u64,
    first: u64,
    frames: u16,
    rate: u32,
    time: f64,
    interval: f64,
    send_age_ms: f64,
    send_interval_ms: f64,
    arrival_excess_ms: f64,
    kernel_queue_ms: f64,
    interface_index: u32,
    peak: f32,
    peer: SocketAddr,
}
struct Hist {
    bins: Vec<u64>,
    count: u64,
    max: f64,
    resolution: f64,
}
impl Hist {
    fn new(resolution: f64) -> Self {
        Self {
            resolution,
            bins: vec![0; 10001],
            count: 0,
            max: 0.,
        }
    }
    fn add(&mut self, x: f64) {
        if x <= 0. {
            return;
        }
        self.bins[((x / self.resolution).round() as usize).min(10000)] += 1;
        self.count += 1;
        self.max = self.max.max(x);
    }
    fn percentile(&self, p: f64) -> f64 {
        let target = (self.count as f64 * p).ceil() as u64;
        let mut n = 0;
        for (i, v) in self.bins.iter().enumerate() {
            n += v;
            if n >= target {
                return i as f64 * self.resolution;
            }
        }
        0.
    }
    fn json(&self) -> Value {
        json!({"count":self.count,"p50":self.percentile(0.5),"p95":self.percentile(0.95),"p99":self.percentile(0.99),"max":self.max,"resolution_ms":self.resolution,"histogram_cap_ms":10000.*self.resolution})
    }
}
fn emit(value: Value, file: &mut Option<std::fs::File>) -> anyhow::Result<()> {
    let line = serde_json::to_string(&value)?;
    println!("{line}");
    if let Some(f) = file {
        writeln!(f, "{line}")?;
    }
    Ok(())
}
pub fn run() -> anyhow::Result<()> {
    let a = Args::parse();
    let mut devices = [Device {
        id: 0,
        channels: 0,
        buffer_frames: 0,
        latency_frames: 0,
        safety_frames: 0,
        rate: 0.,
        name: [0; 256],
    }; 64];
    let count = unsafe { lan_devices(devices.as_mut_ptr(), 64) };
    ensure!(count >= 0, "CoreAudio device enumeration failed");
    let devices = &devices[..count as usize];
    if a.list_devices {
        for d in devices {
            println!("{}", d.json());
        }
        return Ok(());
    }
    ensure!(
        a.buffer_ms.is_finite() && (2. ..=100.).contains(&a.buffer_ms),
        "buffer-ms must be 2..100"
    );
    ensure!(
        a.av_sync_delay_ms.is_finite() && (0. ..=250.).contains(&a.av_sync_delay_ms),
        "AV delay must be 0..250"
    );
    ensure!(
        a.gain_db.is_finite() && (-96. ..=0.).contains(&a.gain_db),
        "gain-db must be -96..0"
    );
    ensure!(
        a.seconds.is_finite() && a.seconds >= 0.,
        "seconds must be nonnegative"
    );
    validate_multicast(&a)?;
    let matches: Vec<_> = devices
        .iter()
        .filter(|d| d.name().contains(&a.device) || d.id.to_string() == a.device)
        .collect();
    let mut device = if !a.diagnostic {
        ensure!(
            matches.len() == 1,
            "Output device must match exactly one device; use --list-devices (matches: {})",
            matches.len()
        );
        let d = *matches[0];
        ensure!(
            a.channel >= 1 && a.channel < d.channels,
            "Output channel pair is out of range"
        );
        d
    } else {
        Device {
            id: 0,
            channels: 2,
            buffer_frames: 480,
            latency_frames: 0,
            safety_frames: 0,
            rate: 48000.,
            name: [0; 256],
        }
    };
    let mut file = a
        .output
        .as_ref()
        .map(|path| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        })
        .transpose()
        .context("Cannot create telemetry file (existing files are never overwritten)")?;
    let epoch = Instant::now();
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    ctrlc::set_handler(move || {
        flag.store(false, Ordering::Relaxed);
    })?;
    let packets = Arc::new(ArrayQueue::new(256));
    let snapshots = Arc::new(ArrayQueue::new(4096));
    let net_events = Arc::new(ArrayQueue::new(8192));
    let counters = Arc::new(NetworkStats::default());
    let scheduling = Arc::new(ArrayQueue::new(1));
    let telemetry_dropped = Arc::new(AtomicU64::new(0));
    let last_callback_ns = Arc::new(AtomicU64::new(0));
    let mut state = Box::new(RenderState {
        last_callback_ns: last_callback_ns.clone(),
        playout: Playout::new(device.rate, a.buffer_ms, a.av_sync_delay_ms, !a.no_asrc),
        packets: packets.clone(),
        snapshots: snapshots.clone(),
        epoch,
        last: 0.,
        gain: 10f32.powf(a.gain_db / 20.),
        telemetry_dropped: telemetry_dropped.clone(),
    });
    let mut error = 0;
    let output = if a.diagnostic {
        None
    } else {
        let ptr = unsafe {
            lan_start(
                &mut device,
                a.channel - 1,
                a.buffer_frames,
                render,
                (&mut *state as *mut RenderState).cast(),
                &mut error,
            )
        };
        if ptr.is_null() {
            bail!("CoreAudio output failed: OSStatus {error}");
        }
        Some(Output(ptr))
    };
    // Bind after AUHAL startup: otherwise the kernel queues initialization-time PCM.
    let socket = UdpSocket::bind(a.bind).with_context(|| format!("Cannot bind {}", a.bind))?;
    if let Some(group) = a.multicast_group {
        socket.join_multicast_v4(&group, &a.multicast_interface)
            .with_context(|| format!("Cannot join multicast group {group} on {}; specify this Mac's LAN IPv4 with --multicast-interface", a.multicast_interface))?;
    }
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let timestamp_status = unsafe { lan_prepare_socket(socket.as_raw_fd()) };
    emit(
        json!({"event":"receiver_start","schema_version":1,"kernel_timestamp_status":timestamp_status,"network_scheduling":format!("{:?}",a.network_scheduling),"bind":a.bind.to_string(),"source_filter":a.source.map(|p|p.to_string()),"multicast_group":a.multicast_group.map(|p|p.to_string()),"multicast_interface":a.multicast_group.map(|_|a.multicast_interface.to_string()),"device":device.json(),"channel_pair":[a.channel,a.channel+1],"target_buffer_ms":a.buffer_ms,"av_sync_delay_ms":a.av_sync_delay_ms,"gain_db":a.gain_db,"asrc":!a.no_asrc,"asrc_quality":"linear baseline","diagnostic":a.diagnostic,"one_way_latency_ms":null}),
        &mut file,
    )?;
    let net_thread = {
        let running = running.clone();
        let counters = counters.clone();
        let telemetry_dropped = telemetry_dropped.clone();
        let events = net_events.clone();
        let packets = packets.clone();
        let scheduling = scheduling.clone();
        std::thread::spawn(move || -> std::io::Result<()> {
            // Fault in packet/session storage before requesting deadline scheduling.
            let mut bytes = [0u8; 65536];
            let mut retired = [None; 256];
            let mut retired_next = 0usize;
            let mut report = Scheduling::default();
            unsafe {
                lan_network_priority(
                    matches!(a.network_scheduling, NetworkScheduling::Realtime) as i32,
                    &mut report,
                )
            };
            let _ = scheduling.push(report);
            let mut peer_ip = a.source;
            let mut last = 0.;
            let mut last_sent = 0u64;
            let mut current = None;

            let mut max_acquired = 0;
            while running.load(Ordering::Relaxed) {
                let mut receipt = Receipt::default();
                let count = unsafe {
                    lan_receive(
                        socket.as_raw_fd(),
                        bytes.as_mut_ptr(),
                        bytes.len(),
                        &mut receipt,
                    )
                };
                if count < 0 {
                    let e = std::io::Error::last_os_error();
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) {
                        continue;
                    }
                    return Err(e);
                }
                let n = count as usize;
                let ip = if receipt.ipv6 != 0 {
                    IpAddr::V6(Ipv6Addr::from(receipt.address))
                } else {
                    IpAddr::V4(Ipv4Addr::new(
                        receipt.address[0],
                        receipt.address[1],
                        receipt.address[2],
                        receipt.address[3],
                    ))
                };
                let peer = SocketAddr::new(ip, receipt.port);
                let now = epoch.elapsed().as_secs_f64();
                if peer_ip.is_some_and(|ip| ip != peer.ip()) {
                    counters.foreign.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let p = match Packet::decode(
                    &bytes[..n],
                    if (0. ..10_000.).contains(&receipt.kernel_queue_ms) {
                        now - receipt.kernel_queue_ms / 1000.
                    } else {
                        now
                    },
                ) {
                    Ok(p) if p.stream == a.stream && p.channels <= 2 => p,
                    _ => {
                        counters.invalid.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                if current != Some(p.session) {
                    if retired.contains(&Some(p.session))
                        || (current.is_some() && p.acquired < max_acquired)
                    {
                        counters.retired.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    if let Some(old) = current {
                        retired[retired_next] = Some(old);
                        retired_next = (retired_next + 1) % retired.len();
                    }
                    current = Some(p.session);
                }
                max_acquired = max_acquired.max(p.acquired);
                peer_ip = Some(peer.ip());
                counters.received.fetch_add(1, Ordering::Relaxed);
                // Publish PCM before optional level and log metadata work.
                if packets
                    .push(QueuedPacket {
                        packet: p,
                        queued_at: epoch.elapsed().as_secs_f64(),
                    })
                    .is_err()
                {
                    counters.queue_dropped.fetch_add(1, Ordering::Relaxed);
                }
                let peak = p.samples[..p.frames as usize * p.channels as usize]
                    .iter()
                    .fold(0f32, |a, b| a.max(b.abs()));
                let send_interval_ms = if last_sent > 0 {
                    (p.sent as i128 - last_sent as i128) as f64 / 10000.
                } else {
                    0.
                };
                let event = NetEvent {
                    kernel_queue_ms: receipt.kernel_queue_ms,
                    interface_index: receipt.interface_index,
                    session: p.session,
                    sequence: p.sequence,
                    first: p.first,
                    frames: p.frames,
                    rate: p.rate,
                    time: now,
                    interval: if last > 0. { (now - last) * 1000. } else { 0. },
                    send_interval_ms,
                    arrival_excess_ms: if last > 0. {
                        (now - last) * 1000. - send_interval_ms
                    } else {
                        0.
                    },
                    send_age_ms: p.sent.saturating_sub(p.acquired) as f64 / 10000.,
                    peak,
                    peer,
                };
                last = now;
                last_sent = p.sent;
                if events.push(event).is_err() {
                    telemetry_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(())
        })
    };
    let mut interval = Hist::new(0.05);
    let mut work = Hist::new(0.001);
    let mut arrivals = Hist::new(0.05);
    let mut send_age = Hist::new(0.05);
    let mut send_intervals = Hist::new(0.05);
    let mut arrival_excess = Hist::new(0.05);
    let mut kernel_queue = Hist::new(0.05);
    let mut handoff = Hist::new(0.01);
    let mut handoff_packets = 0usize;
    let mut latest = None;
    let mut last_net = None;
    let mut peak = 0f32;
    let mut next_summary = 1.;
    let mut dummy = [0f32; 960];
    let mut output_failed = false;
    while running.load(Ordering::Relaxed) {
        let now = epoch.elapsed().as_secs_f64();
        if a.diagnostic {
            render(
                (&mut *state as *mut RenderState).cast(),
                dummy.as_mut_ptr(),
                480,
                0.,
            );
        }
        if let Some(r) = scheduling.pop() {
            let ns_per_tick = r.numer as f64 / r.denom as f64;
            emit(
                json!({"event":"receiver_scheduling","requested":format!("{:?}",a.network_scheduling),"qos_status":r.qos_status,"set_status":r.set_status,"get_status":r.get_status,"is_default":r.is_default!=0,"period_ms":r.period as f64*ns_per_tick/1e6,"computation_ms":r.computation as f64*ns_per_tick/1e6,"constraint_ms":r.constraint as f64*ns_per_tick/1e6}),
                &mut file,
            )?;
        }
        while let Some(s) = snapshots.pop() {
            handoff.add(s.handoff_max_ms);
            handoff_packets += s.handoff_packets;
            interval.add(s.interval_ms);
            work.add(s.work_ms);
            if a.events && s.stats.sessions > 0 {
                emit(
                    json!({"event":"playout","session_id":s.stats.session_id,"sequence":s.stats.playout_sequence,"first_sample":s.stats.render_first_sample,"render_time_ns":(s.time*1e9)as u64,"output_frames":s.frames,"buffer_depth_ms":s.stats.fill_ms,"presentation_lead_ms":s.presentation_lead_ms}),
                    &mut file,
                )?;
            }
            latest = Some(s);
        }
        while let Some(n) = net_events.pop() {
            arrivals.add(n.interval);
            send_age.add(n.send_age_ms);
            send_intervals.add(n.send_interval_ms);
            arrival_excess.add(n.arrival_excess_ms);
            kernel_queue.add(n.kernel_queue_ms);
            peak = peak.max(n.peak);
            last_net = Some(n);
            if a.events {
                emit(
                    json!({"event":"receive","session_id":n.session,"sequence":n.sequence,"first_sample":n.first,"frames":n.frames,"receive_time_ns":(n.time*1e9) as u64,"sender_capture_to_send_ms":n.send_age_ms,"sender_send_interval_ms":n.send_interval_ms,"arrival_excess_ms":n.arrival_excess_ms,"kernel_queue_ms":n.kernel_queue_ms,"interface_index":n.interface_index}),
                    &mut file,
                )?;
            }
        }
        let finish = a.seconds > 0. && now >= a.seconds;
        if now >= next_summary || finish {
            let stats = latest.map(|s| s.stats).unwrap_or_default();
            emit(
                json!({"event":if finish{"receiver_final"}else{"receiver_summary"},"elapsed_seconds":now,"status":if last_net.is_none_or(|n|now-n.time>0.5){"waiting"}else{"receiving"},"peer":last_net.map(|n|n.peer.to_string()),"sample_rate":last_net.map(|n|n.rate),"received_packets":counters.received.load(Ordering::Relaxed),"invalid_packets":counters.invalid.load(Ordering::Relaxed),"foreign_packets":counters.foreign.load(Ordering::Relaxed),"queue_dropped":counters.queue_dropped.load(Ordering::Relaxed),"retired_session_packets":counters.retired.load(Ordering::Relaxed),"telemetry_dropped":telemetry_dropped.load(Ordering::Relaxed),"network_peak":peak,"playout":stats,"arrival_interval_ms":arrivals.json(),"sender_capture_to_send_ms":send_age.json(),"sender_send_interval_ms":send_intervals.json(),"arrival_excess_positive_ms":arrival_excess.json(),"kernel_queue_ms":kernel_queue.json(),"handoff_batch_max_ms":handoff.json(),"handoff_packets":handoff_packets,"interface_index":last_net.map(|n|n.interface_index),"callback_interval_ms":interval.json(),"callback_work_ms":work.json(),"callback_frames":latest.map(|s|s.frames),"presentation_lead_ms":latest.map(|s|s.presentation_lead_ms),"last_callback_seconds":latest.map(|s|s.time),"target_buffer_ms":a.buffer_ms,"av_sync_delay_ms":a.av_sync_delay_ms,"one_way_latency_ms":null}),
                &mut file,
            )?;
            next_summary = now + 1.;
        }
        if !a.diagnostic
            && now > 1.
            && now - last_callback_ns.load(Ordering::Relaxed) as f64 / 1e9 > 0.5
        {
            output_failed = true;
            running.store(false, Ordering::Relaxed);
            break;
        }
        if finish || net_thread.is_finished() {
            running.store(false, Ordering::Relaxed);
            break;
        }
        std::thread::sleep(Duration::from_millis(if a.diagnostic { 10 } else { 50 }));
    }
    running.store(false, Ordering::Relaxed);
    drop(output);
    net_thread
        .join()
        .map_err(|_| anyhow::anyhow!("network thread panicked"))??;
    emit(
        json!({"event":"receiver_stopped","elapsed_seconds":epoch.elapsed().as_secs_f64()}),
        &mut file,
    )?;
    if output_failed {
        bail!("CoreAudio callbacks stopped. Reconnect the selected device and restart reception.");
    }
    Ok(())
}

fn validate_multicast(a: &Args) -> anyhow::Result<()> {
    if let Some(group) = a.multicast_group {
        ensure!(
            group.is_multicast(),
            "multicast-group must be an IPv4 multicast address (224.0.0.0–239.255.255.255)"
        );
        ensure!(
            a.bind.ip() == IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            "Multicast reception requires --bind 0.0.0.0:PORT; use --multicast-interface for this Mac's LAN IPv4"
        );
        ensure!(
            a.multicast_interface.is_unspecified()
                || (!a.multicast_interface.is_multicast()
                    && a.multicast_interface != Ipv4Addr::BROADCAST),
            "multicast-interface must be this Mac's unicast IPv4 address or 0.0.0.0"
        );
        ensure!(
            a.source
                .is_none_or(|ip| ip.is_ipv4() && !ip.is_multicast() && !ip.is_unspecified()),
            "--source must be the Windows sender's unicast IPv4, not the multicast group"
        );
    }
    Ok(())
}

#[cfg(test)]
mod multicast_tests {
    use super::*;

    #[test]
    fn accepts_unicast_and_multicast_configuration() {
        for args in [
            vec!["receiver"],
            vec!["receiver", "--multicast-group", "239.255.0.1"],
            vec![
                "receiver",
                "--multicast-group",
                "239.255.0.1",
                "--multicast-interface",
                "192.168.11.65",
                "--source",
                "192.168.11.10",
            ],
        ] {
            validate_multicast(&Args::try_parse_from(args).unwrap()).unwrap();
        }
    }

    #[test]
    fn rejects_invalid_membership_before_audio_start() {
        for args in [
            vec!["receiver", "--multicast-group", "192.168.11.65"],
            vec![
                "receiver",
                "--multicast-group",
                "239.255.0.1",
                "--bind",
                "[::]:40100",
            ],
            vec![
                "receiver",
                "--multicast-group",
                "239.255.0.1",
                "--bind",
                "192.168.11.65:40100",
            ],
            vec![
                "receiver",
                "--multicast-group",
                "239.255.0.1",
                "--source",
                "239.255.0.1",
            ],
            vec![
                "receiver",
                "--multicast-group",
                "239.255.0.1",
                "--multicast-interface",
                "239.255.0.1",
            ],
        ] {
            assert!(validate_multicast(&Args::try_parse_from(args).unwrap()).is_err());
        }
        assert!(
            Args::try_parse_from(["receiver", "--multicast-interface", "192.168.11.65"]).is_err()
        );
    }
}
