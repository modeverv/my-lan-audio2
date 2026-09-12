#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod report;
#[cfg(windows)]
mod udp;

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Backend {
    Legacy,
    Min,
    Auto,
    ProcessExclude,
    ProcessInclude,
    Ks,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Tap {
    Default,
    Pre,
    Post,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Mute {
    Keep,
    On,
    Off,
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Event-driven WASAPI capture diagnostics and optional bounded UDP PCM sender"
)]
pub struct Options {
    /// Record bounded UDP metadata (no PCM) in --output for cross-host correlation.
    #[arg(long, requires = "output", requires = "udp_to")]
    events: bool,
    /// Parent GUI control: stop gracefully on a 'stop' line or stdin EOF.
    #[arg(long, hide = true)]
    control_stdin: bool,
    /// Explicit UDP destination; omitted means diagnostics only. PCM is unencrypted.
    #[arg(long)]
    udp_to: Option<std::net::SocketAddr>,
    /// Competing nonblocking network workers (packets may arrive out of order).
    #[arg(long, default_value_t=1, value_parser=clap::value_parser!(u8).range(1..=3))]
    udp_workers: u8,
    /// Drop packets older than this many milliseconds since PCM acquisition.
    #[arg(long, default_value_t=5, value_parser=clap::value_parser!(u32).range(1..=1000))]
    udp_deadline_ms: u32,
    /// Maximum frames in each datagram; never wait for a full packet.
    #[arg(long, default_value_t=128, value_parser=clap::value_parser!(u16).range(1..=1024))]
    udp_frames: u16,
    #[arg(long, default_value_t = 1)]
    stream_id: u32,
    /// Measurement duration in seconds (1..86400).
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    seconds: u64,
    /// Auto attempts IAudioClient3 minimum and records any fallback; min fails strictly.
    #[arg(long, value_enum, default_value_t = Backend::Auto)]
    backend: Backend,
    /// Timing-only JSONL output, opened before capture. Existing files are not overwritten.
    #[arg(long)]
    output: Option<PathBuf>,
    /// List active render endpoints and exit.
    #[arg(long)]
    list_devices: bool,
    /// Select an endpoint ID from --list-devices instead of the default multimedia endpoint.
    #[arg(long)]
    device: Option<String>,
    /// Generate a quiet 440 Hz test tone (-46 dBFS) on the selected endpoint.
    #[arg(long)]
    test_tone: bool,
    /// Process include target; exclude defaults to this capture process (and children).
    #[arg(long)]
    pid: Option<u32>,
    #[arg(long, value_enum, default_value_t = Tap::Default)]
    tap: Tap,
    /// Temporarily set endpoint mute during measurement; restore original value on exit.
    #[arg(long, value_enum, default_value_t = Mute::Keep)]
    endpoint_mute: Mute,
    /// Read PCM only to report peak amplitude (no PCM storage).
    #[arg(long)]
    measure_level: bool,
    /// Standalone source process for process-loopback benchmarks.
    #[arg(long)]
    tone_only: bool,
    /// Source IAudioClient3 render period; 0 requests supported minimum; try 48 for 1ms at 48k.
    #[arg(long)]
    render_period_frames: Option<u32>,
    /// Request RAW processing on the independent render stream.
    #[arg(long)]
    render_raw: bool,
    /// Emit/detect 20ms tone pulses every 500ms for same-host source-to-capture timing.
    #[arg(long)]
    pulse_probe: bool,
    /// Read driver KS pin categories and loopback tap capability replies; no driver writes.
    #[arg(long)]
    ks_inspect: bool,
    #[arg(long)]
    ks_filter: Option<String>,
    #[arg(long)]
    ks_pin: Option<u32>,
    /// Requested frames per DMA notification (two notifications per cyclic buffer).
    #[arg(long, default_value_t=48, value_parser=clap::value_parser!(u32).range(1..=4096))]
    ks_frames: u32,
    /// Permit legacy cursor fallback; cannot prove complete audio or count ring wraps.
    #[arg(long)]
    ks_position: bool,
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    #[cfg(windows)]
    {
        report::run(options)
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        anyhow::bail!("sender-windows requires Windows 10 1703 or newer");
    }
}
