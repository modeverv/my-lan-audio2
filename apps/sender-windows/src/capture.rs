//! All COM audio interfaces stay on their creating thread. The capture loop only
//! releases PCM and publishes fixed-size timing records to a bounded lock-free queue.
use crate::{Backend, Mute, Options, Tap};
#[path = "kernel.rs"]
mod kernel;
use anyhow::{Context, Result, bail};
use crossbeam_queue::ArrayQueue;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Media::{Audio::*, Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT},
        System::{
            Com::{
                StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
                *,
            },
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::*,
        },
    },
    core::{Interface, PCWSTR, PWSTR, w},
};

struct Com;
impl Com {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}

#[windows::core::implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationDone(std::sync::mpsc::Sender<()>);
impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationDone_Impl {
    fn ActivateCompleted(
        &self,
        _: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

fn activate_process(pid: u32, include: bool) -> Result<IAudioClient> {
    use windows::Win32::System::{Com::StructuredStorage::*, Variant::VT_BLOB};
    unsafe {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: if include {
                        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
                    } else {
                        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
                    },
                },
            },
        };
        // Borrowed blob: never PropVariantClear this stack-backed variant.
        let variant = std::mem::ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: std::mem::size_of_val(&params) as u32,
                            pBlobData: (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
                        },
                    },
                    ..Default::default()
                }),
            },
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = ActivationDone(tx).into();
        let operation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*variant),
            &handler,
        )?;
        rx.recv_timeout(Duration::from_secs(10))
            .context("process-loopback activation timeout")?;
        let mut status = windows::core::HRESULT(0);
        let mut unknown = None;
        operation.GetActivateResult(&mut status, &mut unknown)?;
        status.ok()?;
        Ok(unknown
            .context("activation returned null interface")?
            .cast()?)
    }
}

struct RestoreMute {
    volume: windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
    original: bool,
}
impl Drop for RestoreMute {
    fn drop(&mut self) {
        unsafe {
            let _ = self.volume.SetMute(self.original, std::ptr::null());
        }
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct EventHandle(HANDLE);
impl EventHandle {
    fn new() -> Result<Self> {
        Ok(Self(unsafe { CreateEventW(None, false, false, None)? }))
    }
}
impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct MixFormat(*mut WAVEFORMATEX);
impl MixFormat {
    fn process_mix() -> Result<Self> {
        // Process loopback accepts a requested format independently of render endpoints.
        let ptr =
            unsafe { CoTaskMemAlloc(std::mem::size_of::<WAVEFORMATEX>()) }.cast::<WAVEFORMATEX>();
        anyhow::ensure!(!ptr.is_null(), "allocate process mix format");
        unsafe {
            ptr.write(WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: 2,
                nSamplesPerSec: 48_000,
                nAvgBytesPerSec: 48_000 * 8,
                nBlockAlign: 8,
                wBitsPerSample: 32,
                cbSize: 0,
            });
        }
        Ok(Self(ptr))
    }
}
impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}

struct Mmcss(HANDLE);
impl Drop for Mmcss {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

struct Started(IAudioClient);
impl Drop for Started {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

fn take_string(ptr: PWSTR) -> Result<String> {
    let result = unsafe { ptr.to_string() };
    unsafe {
        CoTaskMemFree(Some(ptr.0.cast()));
    }
    Ok(result?)
}

fn device_info(device: &IMMDevice) -> Result<(String, String)> {
    unsafe {
        let id = take_string(device.GetId()?)?;
        let store = device.OpenPropertyStore(STGM_READ)?;
        let mut prop = store.GetValue(&PKEY_Device_FriendlyName)?;
        let name_result = PropVariantToStringAlloc(&prop);
        PropVariantClear(&mut prop)?;
        Ok((id, take_string(name_result?)?))
    }
}

fn select_device(id: Option<&str>) -> Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        Ok(if let Some(id) = id {
            let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
            enumerator.GetDevice(PCWSTR(wide.as_ptr()))?
        } else {
            enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?
        })
    }
}

pub fn inspect_ks(id: Option<&str>) -> Result<()> {
    use windows::Win32::Media::KernelStreaming::*;
    use windows::core::GUID;
    let _com = Com::new()?;
    let endpoint = select_device(id)?;
    unsafe {
        let topology: IDeviceTopology = endpoint.Activate(CLSCTX_ALL, None)?;
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let mut adapters = Vec::new();
        let mut pending = Vec::new();
        let mut visited = std::collections::HashSet::new();
        for connector in 0..topology.GetConnectorCount()? {
            pending.push(take_string(
                topology.GetConnector(connector)?.GetDeviceIdConnectedTo()?,
            )?);
        }
        while let Some(adapter_id) = pending.pop() {
            if !visited.insert(adapter_id.clone()) {
                continue;
            }
            anyhow::ensure!(visited.len() <= 32, "unexpectedly large audio topology");
            let wide: Vec<u16> = adapter_id.encode_utf16().chain(Some(0)).collect();
            let adapter = enumerator.GetDevice(PCWSTR(wide.as_ptr()))?;
            let ks: IKsControl = match adapter.Activate(CLSCTX_ALL, None) {
                Ok(ks) => ks,
                Err(error) => {
                    adapters.push(
                        json!({"adapter_id":adapter_id,"ks_activation_error":error.to_string()}),
                    );
                    continue;
                }
            };
            if let Ok(next_topology) = adapter.Activate::<IDeviceTopology>(CLSCTX_ALL, None) {
                for index in 0..next_topology.GetConnectorCount()? {
                    if let Ok(next_id) = next_topology.GetConnector(index)?.GetDeviceIdConnectedTo()
                    {
                        pending.push(take_string(next_id)?);
                    }
                }
            }
            let make_property = |set: GUID, id: u32| KSIDENTIFIER {
                Anonymous: KSIDENTIFIER_0 {
                    Anonymous: KSIDENTIFIER_0_0 {
                        Set: set,
                        Id: id,
                        Flags: KSPROPERTY_TYPE_GET,
                    },
                },
            };
            let property = make_property(KSPROPSETID_Pin, KSPROPERTY_PIN_CTYPES.0 as u32);
            let (mut count, mut returned) = (0u32, 0u32);
            ks.KsProperty(
                &property,
                std::mem::size_of_val(&property) as u32,
                (&mut count as *mut u32).cast(),
                4,
                &mut returned,
            )?;
            anyhow::ensure!(count <= 256, "unexpected KS pin count {count}");
            let mut pins = Vec::new();
            for pin in 0..count {
                let mut request = KSP_PIN {
                    Property: make_property(KSPROPSETID_Pin, KSPROPERTY_PIN_CATEGORY.0 as u32),
                    PinId: pin,
                    ..Default::default()
                };
                let mut category = GUID::zeroed();
                let category_result = ks.KsProperty(
                    &request.Property,
                    std::mem::size_of_val(&request) as u32,
                    (&mut category as *mut GUID).cast(),
                    16,
                    &mut returned,
                );
                let mut flow = 0u32;
                request.Property = make_property(KSPROPSETID_Pin, KSPROPERTY_PIN_DATAFLOW.0 as u32);
                let flow_result = ks.KsProperty(
                    &request.Property,
                    std::mem::size_of_val(&request) as u32,
                    (&mut flow as *mut u32).cast(),
                    4,
                    &mut returned,
                );
                let mut caps = 0u32;
                request.Property =
                    make_property(GUID::from_u128(0xb3648bc8_5b91_468a_b94d_f4641250917c), 0);
                let caps_result = ks.KsProperty(
                    &request.Property,
                    std::mem::size_of_val(&request) as u32,
                    (&mut caps as *mut u32).cast(),
                    4,
                    &mut returned,
                );
                pins.push(json!({"pin":pin,"category":category_result.as_ref().ok().map(|_|format!("{category:?}")),
                    "category_error":category_result.err().map(|e|e.to_string()),"dataflow":flow_result.as_ref().ok().map(|_|flow),
                    "dataflow_error":flow_result.err().map(|e|e.to_string()),
                    "loopback_tap_caps":caps_result.as_ref().ok().map(|_|caps),"loopback_tap_caps_error":caps_result.err().map(|e|e.to_string())}));
            }
            adapters.push(json!({"adapter_id":adapter_id,"pins":pins}));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"event":"ks_inspection","endpoint":device_info(&endpoint)?.1,
            "note":"Read-only filter pin queries through IKsControl; no streaming pin opened, no kernel capture latency measurement", "adapters":adapters})
            )?
        );
    }
    Ok(())
}

pub fn list_devices() -> Result<()> {
    let _com = Com::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()
            .and_then(|d| device_info(&d).ok())
            .map(|(id, _)| id);
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for index in 0..devices.GetCount()? {
            let (id, name) = device_info(&devices.Item(index)?)?;
            println!(
                "{}",
                json!({"name":name,"id":id,"default_multimedia":default_id.as_ref()==Some(&id)})
            );
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Record {
    Udp {
        #[serde(flatten)]
        packet: crate::udp::PacketEvent,
    },
    KernelPosition {
        time_100ns: u64,
        play_offset: u64,
        write_offset: u64,
    },
    RenderWake {
        interval_us: Option<u64>,
        frames: u32,
    },
    SourcePulse {
        time_before_release_100ns: u64,
        time_after_release_100ns: u64,
        source_frame: u64,
        padding_frames: u32,
    },
    Pulse {
        capture_time_100ns: u64,
        first_sample: u64,
        offset_frames: u32,
    },
    Capture {
        wake_time_100ns: Option<u64>,
        get_buffer_start_100ns: Option<u64>,
        sequence: u64,
        first_sample: u64,
        device_position: u64,
        device_position_valid: bool,
        frames: u32,
        flags: u32,
        capture_time_100ns: u64,
        first_sample_qpc_100ns: Option<u64>,
        first_sample_age_us: Option<i64>,
        get_buffer_us: u64,
        capture_to_record_us: u64,
        peak: Option<f32>,
    },
    Wake {
        time_100ns: u64,
        interval_us: Option<u64>,
        frames: u64,
        packets: u32,
        drain_us: u64,
    },
}

pub struct Shared {
    pub queue: ArrayQueue<Record>,
    pub dropped: AtomicU64,
    pub stop: AtomicBool,
}
impl Shared {
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(capacity),
            dropped: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        }
    }
    pub(crate) fn push(&self, record: Record) {
        if self.queue.push(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Qpc {
    frequency: u64,
}
impl Qpc {
    fn new() -> Result<Self> {
        let mut frequency = 0;
        unsafe {
            QueryPerformanceFrequency(&mut frequency)?;
        }
        Ok(Self {
            frequency: frequency as u64,
        })
    }
    fn now(&self) -> u64 {
        let mut value = 0;
        // QPC cannot fail on the supported Windows versions.
        unsafe {
            QueryPerformanceCounter(&mut value).expect("QPC unavailable");
        }
        ((value as u128 * 10_000_000) / self.frequency as u128) as u64
    }
}

pub(crate) fn thread_cpu_100ns() -> Result<u64> {
    let (mut creation, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )?;
    }
    let value = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    Ok(value(kernel) + value(user))
}

fn format_info(format: &MixFormat) -> Value {
    unsafe {
        let f = *format.0;
        let subformat = if f.wFormatTag == 0xfffe && f.cbSize >= 22 {
            let guid = (*format.0.cast::<WAVEFORMATEXTENSIBLE>()).SubFormat;
            Some(format!("{guid:?}"))
        } else {
            None
        };
        let (rate, channels, bits, align, tag) = (
            f.nSamplesPerSec,
            f.nChannels,
            f.wBitsPerSample,
            f.nBlockAlign,
            f.wFormatTag,
        );
        json!({"sample_rate":rate,"channels":channels,"bits_per_sample":bits,
            "block_align":align,"format_tag":tag,"subformat":subformat,
            "sample_format":if is_float(format) {"f32_le"} else {"native (timing-only, no conversion)"}})
    }
}

fn is_float(format: &MixFormat) -> bool {
    unsafe {
        let f = *format.0;
        let subformat = if f.wFormatTag == 0xfffe && f.cbSize >= 22 {
            Some((*format.0.cast::<WAVEFORMATEXTENSIBLE>()).SubFormat)
        } else {
            None
        };
        f.wBitsPerSample == 32
            && (f.wFormatTag == 3
                || (f.wFormatTag == 0xfffe
                    && f.cbSize >= 22
                    && subformat == Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT)))
    }
}

fn min_initialize(client: &IAudioClient, format: &MixFormat, flags: u32) -> Result<u32> {
    unsafe {
        let client3: IAudioClient3 = client.cast().context("QueryInterface IAudioClient3")?;
        let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
        client3
            .GetSharedModeEnginePeriod(format.0, &mut default, &mut fundamental, &mut min, &mut max)
            .context("GetSharedModeEnginePeriod")?;
        client3
            .InitializeSharedAudioStream(flags, min, format.0, None)
            .with_context(|| format!("InitializeSharedAudioStream requested {min} frames"))?;
        Ok(min)
    }
}

pub fn run(options: &Options, shared: &Arc<Shared>, startup: Sender<Value>) -> Result<Value> {
    if matches!(options.backend, Backend::Ks) {
        return kernel::run(options, shared, startup);
    }
    let _com = Com::new()?;
    let device = select_device(options.device.as_deref()).context("select render endpoint")?;
    let (device_id, device_name) = device_info(&device)?;
    // Windows owns PCM; MixFormat owns the COM-allocated format. Both survive the loop.
    unsafe {
        let volume: windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume =
            device.Activate(CLSCTX_ALL, None)?;
        let original_mute = volume.GetMute()?.as_bool();
        let _restore_mute = if !matches!(options.endpoint_mute, Mute::Keep) {
            let restore = RestoreMute {
                volume: volume.clone(),
                original: original_mute,
            };
            volume.SetMute(matches!(options.endpoint_mute, Mute::On), std::ptr::null())?;
            Some(restore)
        } else {
            None
        };
        let mut client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = if matches!(options.backend, Backend::ProcessExclude) {
            MixFormat::process_mix()?
        } else {
            MixFormat(client.GetMixFormat()?)
        };
        let (mut default_period, mut minimum_period) = (0, 0);
        client.GetDevicePeriod(Some(&mut default_period), Some(&mut minimum_period))?;
        let periods = (|| -> Result<Value> {
            let client3: IAudioClient3 = client.cast()?;
            let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
            client3.GetSharedModeEnginePeriod(
                format.0,
                &mut default,
                &mut fundamental,
                &mut min,
                &mut max,
            )?;
            Ok(
                json!({"default_frames":default,"fundamental_frames":fundamental,"minimum_frames":min,"maximum_frames":max}),
            )
        })();
        let process = matches!(
            options.backend,
            Backend::ProcessInclude | Backend::ProcessExclude
        );
        let target_pid = if matches!(options.backend, Backend::ProcessInclude) {
            Some(
                options
                    .pid
                    .context("--backend process-include requires --pid")?,
            )
        } else if process {
            Some(options.pid.unwrap_or(std::process::id()))
        } else {
            None
        };
        if let Some(pid) = target_pid {
            client = activate_process(pid, matches!(options.backend, Backend::ProcessInclude))?;
        }
        let set_tap = |client: &IAudioClient| -> Result<()> {
            if !matches!(options.tap, Tap::Default) {
                let c2: IAudioClient2 = client.cast()?;
                c2.SetClientProperties(&AudioClientProperties {
                    cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                    eCategory: AudioCategory_Other,
                    Options: AUDCLNT_STREAMOPTIONS(if matches!(options.tap, Tap::Post) {
                        8
                    } else {
                        0
                    }),
                    ..Default::default()
                })?;
            }
            Ok(())
        };
        set_tap(&client).context("SetClientProperties tap request")?;
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | if process {
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            } else {
                0
            };
        let mut fallback_reason = None;
        let requested = match options.backend {
            Backend::Legacy | Backend::ProcessInclude | Backend::ProcessExclude => None,
            Backend::Ks => unreachable!(),
            Backend::Min => Some(min_initialize(&client, &format, flags)?),
            Backend::Auto => match min_initialize(&client, &format, flags) {
                Ok(frames) => Some(frames),
                Err(error) => {
                    fallback_reason = Some(format!("{error:#}"));
                    // Do not reuse a partially initialized audio client after failure.
                    client = device.Activate(CLSCTX_ALL, None)?;
                    set_tap(&client)?;
                    None
                }
            },
        };
        if requested.is_none() {
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, format.0, None)
                .context("legacy event-driven loopback Initialize")?;
        }
        let current_period = (|| -> Result<u32> {
            let client3: IAudioClient3 = client.cast()?;
            let (mut ptr, mut frames) = (std::ptr::null_mut(), 0);
            client3.GetCurrentSharedModeEnginePeriod(&mut ptr, &mut frames)?;
            let _current_format = MixFormat(ptr);
            Ok(frames)
        })();
        let event = EventHandle::new()?;
        client.SetEventHandle(event.0)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        let mut task_index = 0;
        let mmcss_result = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index);
        let mmcss_error = mmcss_result.as_ref().err().map(ToString::to_string);
        let _mmcss = mmcss_result.ok().map(Mmcss);
        let qpc = Qpc::new()?;
        let mut udp = if let Some(destination) = options.udp_to {
            anyhow::ensure!(
                is_float(&format) && (*format.0).nBlockAlign == (*format.0).nChannels * 4,
                "UDP v1 requires native float32 PCM; no implicit conversion"
            );
            Some(crate::udp::Sender::new(
                destination,
                options.udp_workers,
                options.udp_deadline_ms,
                options.udp_frames,
                options.stream_id,
                (*format.0).nSamplesPerSec,
                (*format.0).nChannels,
                options.events.then(|| shared.clone()),
            )?)
        } else {
            None
        };
        let metadata = json!({"event":"startup","device_name":device_name,"device_id":device_id,
            "udp_events":options.events,"udp_destination":options.udp_to.map(|v|v.to_string()),"udp_workers":options.udp_workers,
            "udp_deadline_ms":options.udp_deadline_ms,"udp_max_frames":options.udp_frames,"udp_protocol_version":1,
            "requested_backend":format!("{:?}",options.backend),"selected_backend":if process {"process loopback"} else if requested.is_some(){"IAudioClient3 minimum"}else{"legacy endpoint loopback"},
            "process_target_pid":target_pid,"format_source":if matches!(options.backend, Backend::ProcessExclude) {"fixed 48000 Hz stereo float32; AUTOCONVERTPCM enabled"}else if process {"requested endpoint-reference format; AUTOCONVERTPCM enabled"}else{"native endpoint mix"},
            "fallback_reason":fallback_reason,"format":format_info(&format),
            "device_default_period_100ns":default_period,"device_minimum_period_100ns":minimum_period,
            "shared_engine_periods":periods.as_ref().ok(),"shared_engine_period_query_error":periods.as_ref().err().map(|e|format!("{e:#}")),
            "requested_period_frames":if matches!(options.backend, Backend::Legacy) || process { None } else { periods.as_ref().ok().and_then(|v|v["minimum_frames"].as_u64()) },
            "accepted_period_frames":requested,"current_engine_period_frames":current_period.as_ref().ok(),
            "current_engine_period_error":current_period.as_ref().err().map(|e|format!("{e:#}")),
            "capture_buffer_frames":client.GetBufferSize()?,"reported_stream_latency_100ns":client.GetStreamLatency().ok(),
            "tap_mode":format!("{:?}",options.tap),"pre_volume_pre_mute_verified":false,
            "tap_note":"Pre requests stream options NONE; Post requests POST_VOLUME_LOOPBACK (0x8). Verify behavior with measured levels.",
            "endpoint_mute_original":original_mute,"endpoint_mute_during_run":volume.GetMute()?.as_bool(),"measure_level":options.measure_level,
            "mmcss_task":"Pro Audio","mmcss_error":mmcss_error,"qpc_frequency":qpc.frequency,
            "duration_requested_seconds":options.seconds,"test_tone":options.test_tone,
            "telemetry_queue_capacity":shared.queue.capacity()});
        startup.send(metadata).context("reporter disconnected")?;
        let cpu_start = thread_cpu_100ns()?;
        client.Start()?;
        let _started = Started(client.clone());
        let start = Instant::now();
        let start_qpc = qpc.now();
        let duration = Duration::from_secs(options.seconds);
        let (mut last_wake, mut sequence, mut first_sample) = (None, 0, 0);
        let mut first_wake = None;
        let mut quiet_frames = 0u64;
        let (mut wake_count, mut empty_wakes, mut timeouts, mut total_frames) =
            (0u64, 0u64, 0u64, 0u64);
        let mut max_packets_per_wake = 0;
        while start.elapsed() < duration && !shared.stop.load(Ordering::Relaxed) {
            // Timeout bounds shutdown latency only. Audio is read exclusively on events.
            let wait_ms = duration
                .saturating_sub(start.elapsed())
                .as_millis()
                .clamp(1, 250) as u32;
            let wait = WaitForSingleObject(event.0, wait_ms);
            if wait == WAIT_TIMEOUT {
                timeouts += 1;
                continue;
            }
            if wait != WAIT_OBJECT_0 {
                bail!("capture event wait failed: {wait:?}");
            }
            let wake = qpc.now();
            first_wake.get_or_insert(wake);
            let interval_us = last_wake.map(|last| (wake - last) / 10);
            last_wake = Some(wake);
            let (mut frames_this_wake, mut packets_this_wake) = (0, 0);
            while capture.GetNextPacketSize()? != 0 {
                let begin = qpc.now();
                let (mut data, mut frames, mut flags, mut position, mut audio_qpc) =
                    (std::ptr::null_mut(), 0, 0, 0, 0);
                capture.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut position),
                    Some(&mut audio_qpc),
                )?;
                if frames == 0 {
                    break;
                }
                let acquired = qpc.now();
                let timestamp_valid =
                    flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0 && audio_qpc != 0;
                if let Some(sender) = udp.as_mut() {
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    if !silent && data.is_null() {
                        capture.ReleaseBuffer(frames)?;
                        bail!("non-silent capture buffer has null PCM pointer");
                    }
                    let bytes = (!silent).then(|| {
                        std::slice::from_raw_parts(
                            data,
                            frames as usize * (*format.0).nBlockAlign as usize,
                        )
                    });
                    sender.publish(
                        bytes,
                        frames,
                        flags,
                        acquired,
                        (!process && timestamp_valid).then_some(position),
                    );
                }
                if options.pulse_probe && is_float(&format) {
                    let channels = (*format.0).nChannels as usize;
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                        quiet_frames += frames as u64;
                    } else if !data.is_null() {
                        let samples = std::slice::from_raw_parts(
                            data.cast::<f32>(),
                            frames as usize * channels,
                        );
                        for (offset, frame) in samples.chunks_exact(channels).enumerate() {
                            if frame.iter().any(|v| v.abs() > 0.0001) {
                                if quiet_frames >= (*format.0).nSamplesPerSec as u64 / 10 {
                                    shared.push(Record::Pulse {
                                        capture_time_100ns: acquired,
                                        first_sample,
                                        offset_frames: offset as u32,
                                    });
                                }
                                quiet_frames = 0;
                            } else {
                                quiet_frames += 1;
                            }
                        }
                    }
                }
                let peak = if options.measure_level && is_float(&format) {
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                        Some(0.0)
                    } else if data.is_null() {
                        None
                    } else {
                        Some(
                            std::slice::from_raw_parts(
                                data.cast::<f32>(),
                                frames as usize * (*format.0).nChannels as usize,
                            )
                            .iter()
                            .fold(0.0f32, |peak, &v| peak.max(v.abs())),
                        )
                    }
                } else {
                    None
                };
                capture.ReleaseBuffer(frames)?;
                let recorded = qpc.now();
                shared.push(Record::Capture {
                    wake_time_100ns: Some(wake),
                    get_buffer_start_100ns: Some(begin),
                    sequence,
                    first_sample,
                    device_position: position,
                    device_position_valid: !process && timestamp_valid,
                    frames,
                    flags,
                    capture_time_100ns: acquired,
                    first_sample_qpc_100ns: timestamp_valid.then_some(audio_qpc),
                    first_sample_age_us: timestamp_valid
                        .then(|| ((recorded as i128 - audio_qpc as i128) / 10) as i64),
                    get_buffer_us: (acquired - begin) / 10,
                    capture_to_record_us: (recorded - acquired) / 10,
                    peak,
                });
                sequence += 1;
                first_sample += frames as u64;
                total_frames += frames as u64;
                frames_this_wake += frames as u64;
                packets_this_wake += 1;
                // Also bound cancellation when a driver continuously provides data.
                if shared.stop.load(Ordering::Relaxed) || start.elapsed() >= duration {
                    break;
                }
            }
            wake_count += 1;
            if packets_this_wake == 0 {
                empty_wakes += 1;
            }
            max_packets_per_wake = max_packets_per_wake.max(packets_this_wake);
            shared.push(Record::Wake {
                time_100ns: wake,
                interval_us,
                frames: frames_this_wake,
                packets: packets_this_wake,
                drain_us: (qpc.now() - wake) / 10,
            });
        }
        client.Stop()?;
        let elapsed = start.elapsed().as_secs_f64();
        let cpu_100ns = thread_cpu_100ns()?.saturating_sub(cpu_start);
        let udp_summary = udp.as_mut().map(crate::udp::Sender::finish);
        Ok(
            json!({"event":"capture_end","elapsed_seconds":elapsed,"start_qpc_100ns":start_qpc,"end_qpc_100ns":qpc.now(),
            "udp":udp_summary,
            "time_to_first_wake_us":first_wake.map(|t|(t-start_qpc)/10),
            "trailing_no_event_us":last_wake.map(|t|(qpc.now()-t)/10),
            "captured_packets":sequence,"captured_frames":total_frames,"wake_count":wake_count,"empty_wakes":empty_wakes,
            "wait_timeouts":timeouts,"max_packets_per_wake":max_packets_per_wake,
            "capture_thread_cpu_ms":cpu_100ns as f64/10_000.0,"capture_thread_cpu_one_core_percent":cpu_100ns as f64/100_000.0/elapsed,
            "telemetry_dropped":shared.dropped.load(Ordering::Relaxed),"cancelled":shared.stop.load(Ordering::Relaxed)}),
        )
    }
}

/// Independent shared-mode renderer supplies repeatable activity. No endpoint
/// volume/default device changes. Its cost is excluded from capture-thread CPU.
pub fn tone(
    device_id: Option<String>,
    shared: Arc<Shared>,
    period: Option<u32>,
    raw: bool,
    seconds: u64,
    pulses: bool,
) -> Result<()> {
    let _com = Com::new()?;
    let device = select_device(device_id.as_deref())?;
    unsafe {
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = MixFormat(client.GetMixFormat()?);
        if !is_float(&format) {
            bail!("test tone currently requires native f32 format; use external playback");
        }
        let rate = (*format.0).nSamplesPerSec;
        let channels = (*format.0).nChannels as usize;
        if raw {
            let c2: IAudioClient2 = client.cast()?;
            c2.SetClientProperties(&AudioClientProperties {
                cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                eCategory: AudioCategory_Other,
                Options: AUDCLNT_STREAMOPTIONS_RAW,
                ..Default::default()
            })?;
        }
        let mut render_periods = serde_json::Value::Null;
        if let Some(requested) = period {
            let c3: IAudioClient3 = client.cast()?;
            let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
            c3.GetSharedModeEnginePeriod(
                format.0,
                &mut default,
                &mut fundamental,
                &mut min,
                &mut max,
            )?;
            let requested = if requested == 0 { min } else { requested };
            render_periods = json!({"default":default,"fundamental":fundamental,"min":min,"max":max,"requested":requested});
            eprintln!("render period probe: {render_periods}");
            c3.InitializeSharedAudioStream(
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                requested,
                format.0,
                None,
            )
            .with_context(|| format!("render InitializeSharedAudioStream {requested} frames"))?;
        } else {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                0,
                0,
                format.0,
                None,
            )?;
        }
        let event = EventHandle::new()?;
        client.SetEventHandle(event.0)?;
        let renderer: IAudioRenderClient = client.GetService()?;
        let capacity = client.GetBufferSize()?;
        eprintln!(
            "{}",
            json!({"event":"source_ready","pid":std::process::id(),"device":device_info(&device)?.1,
            "render_periods":render_periods,"buffer_frames":capacity,"raw":raw,"format":format_info(&format)})
        );
        let mut phase = 0.0f64;
        let mut source_frame = 0u64;
        let source_qpc = Qpc::new()?;
        let mut write = |frames: u32| -> Result<()> {
            if frames == 0 {
                return Ok(());
            }
            let ptr = renderer.GetBuffer(frames)?;
            let padding = client.GetCurrentPadding()?;
            let mut pulse_frame = None;
            let samples =
                std::slice::from_raw_parts_mut(ptr.cast::<f32>(), frames as usize * channels);
            for frame in samples.chunks_exact_mut(channels) {
                if pulses && source_frame.is_multiple_of(rate as u64 / 2) {
                    pulse_frame = Some(source_frame);
                }
                let enabled = !pulses || source_frame % (rate as u64 / 2) < rate as u64 / 50;
                let value = if enabled {
                    (phase.sin() * 0.005) as f32
                } else {
                    0.0
                };
                phase =
                    (phase + std::f64::consts::TAU * 440.0 / rate as f64) % std::f64::consts::TAU;
                frame.fill(value);
                source_frame += 1;
            }
            let before = source_qpc.now();
            renderer.ReleaseBuffer(frames, 0)?;
            if let Some(source_frame) = pulse_frame {
                shared.push(Record::SourcePulse {
                    time_before_release_100ns: before,
                    time_after_release_100ns: source_qpc.now(),
                    source_frame,
                    padding_frames: padding,
                });
            }
            Ok(())
        };
        write(capacity)?;
        client.Start()?;
        let _started = Started(client.clone());
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut last_render_wake = None;
        while !shared.stop.load(Ordering::Relaxed) && Instant::now() < deadline {
            let wait = WaitForSingleObject(event.0, 250);
            if wait == WAIT_TIMEOUT {
                continue;
            }
            if wait != WAIT_OBJECT_0 {
                bail!("test tone wait failed: {wait:?}");
            }
            let now = source_qpc.now();
            let frames = capacity.saturating_sub(client.GetCurrentPadding()?);
            shared.push(Record::RenderWake {
                interval_us: last_render_wake.map(|t| (now - t) / 10),
                frames,
            });
            last_render_wake = Some(now);
            write(frames)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_telemetry_queue_drops_instead_of_waiting() {
        let shared = Shared::new(1);
        let record = Record::Wake {
            time_100ns: 0,
            interval_us: None,
            frames: 0,
            packets: 0,
            drain_us: 0,
        };
        shared.push(record);
        shared.push(record);
        assert_eq!(shared.queue.len(), 1);
        assert_eq!(shared.dropped.load(Ordering::Relaxed), 1);
    }
}
