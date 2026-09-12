//! Direct user-mode KS/WaveRT loopback-pin experiment. No driver installation.
//! Only kernel-provided packet notifications drive reads; PCM is never stored.
use super::*;
use windows::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE},
    Media::KernelStreaming::*,
    Storage::FileSystem::*,
    System::IO::DeviceIoControl,
};
use windows::core::GUID;

fn property(set: GUID, id: u32, flags: u32) -> KSIDENTIFIER {
    KSIDENTIFIER {
        Anonymous: KSIDENTIFIER_0 {
            Anonymous: KSIDENTIFIER_0_0 {
                Set: set,
                Id: id,
                Flags: flags,
            },
        },
    }
}
fn ioctl<T, U>(pin: HANDLE, input: &T, output: &mut U) -> windows::core::Result<()> {
    let mut returned = 0;
    unsafe {
        DeviceIoControl(
            pin,
            IOCTL_KS_PROPERTY,
            Some((input as *const T).cast()),
            std::mem::size_of::<T>() as u32,
            if std::mem::size_of::<U>() == 0 {
                None
            } else {
                Some((output as *mut U).cast())
            },
            std::mem::size_of::<U>() as u32,
            Some(&mut returned),
            None,
        )
    }
}
fn state(pin: HANDLE, state: KSSTATE) -> windows::core::Result<()> {
    let mut value = state.0;
    ioctl(
        pin,
        &property(
            KSPROPSETID_Connection,
            KSPROPERTY_CONNECTION_STATE.0 as u32,
            KSPROPERTY_TYPE_SET,
        ),
        &mut value,
    )
}
struct Stream {
    pin: EventHandle,
    event: HANDLE,
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = state(self.pin.0, KSSTATE_STOP);
        let request = KSRTAUDIO_NOTIFICATION_EVENT_PROPERTY {
            Property: property(
                KSPROPSETID_RtAudio,
                KSPROPERTY_RTAUDIO_UNREGISTER_NOTIFICATION_EVENT.0 as u32,
                KSPROPERTY_TYPE_GET,
            ),
            NotificationEvent: self.event,
        };
        let _ = ioctl(self.pin.0, &request, &mut [] as &mut [u8; 0]);
    }
}

#[repr(C)]
struct ConnectFormat {
    connect: KSPIN_CONNECT,
    format: KSDATAFORMAT,
    wave: WAVEFORMATEXTENSIBLE,
}

pub(super) fn run(options: &Options, shared: &Shared, startup: Sender<Value>) -> Result<Value> {
    anyhow::ensure!(
        !options.test_tone,
        "KS experiment uses a separate --tone-only process"
    );
    let path = options
        .ks_filter
        .as_deref()
        .context("--backend ks requires --ks-filter from --ks-inspect")?;
    let path = path.strip_prefix("{2}.").unwrap_or(path);
    let pin_id = options
        .ks_pin
        .context("--backend ks requires --ks-pin for a loopback-category output pin")?;
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let filter = EventHandle(
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .context("open KS filter")?,
        );
        // Verify the selected factory is a loopback output, never an arbitrary mic/render pin.
        let mut category = GUID::zeroed();
        let request = KSP_PIN {
            Property: property(
                KSPROPSETID_Pin,
                KSPROPERTY_PIN_CATEGORY.0 as u32,
                KSPROPERTY_TYPE_GET,
            ),
            PinId: pin_id,
            ..Default::default()
        };
        ioctl(filter.0, &request, &mut category).context("query selected KS pin category")?;
        anyhow::ensure!(
            category == GUID::from_u128(0x8f42c0b2_91ce_4bcf_9ccd_0e599037ab35),
            "selected pin is not audio loopback"
        );
        let mut flow = 0u32;
        let request = KSP_PIN {
            Property: property(
                KSPROPSETID_Pin,
                KSPROPERTY_PIN_DATAFLOW.0 as u32,
                KSPROPERTY_TYPE_GET,
            ),
            PinId: pin_id,
            ..Default::default()
        };
        ioctl(filter.0, &request, &mut flow)?;
        anyhow::ensure!(
            flow == KSPIN_DATAFLOW_OUT.0 as u32,
            "selected KS pin does not capture data"
        );
        let pcm = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
        let mut connection = ConnectFormat {
            connect: KSPIN_CONNECT {
                Interface: property(
                    KSINTERFACESETID_Standard,
                    KSINTERFACE_STANDARD_LOOPED_STREAMING.0 as u32,
                    0,
                ),
                Medium: property(KSMEDIUMSETID_Standard, KSMEDIUM_TYPE_ANYINSTANCE, 0),
                PinId: pin_id,
                Priority: KSPRIORITY {
                    PriorityClass: KSPRIORITY_NORMAL,
                    PrioritySubClass: 1,
                },
                ..Default::default()
            },
            format: KSDATAFORMAT {
                Anonymous: KSDATAFORMAT_0 {
                    FormatSize: (std::mem::size_of::<KSDATAFORMAT>()
                        + std::mem::size_of::<WAVEFORMATEXTENSIBLE>())
                        as u32,
                    SampleSize: 4,
                    MajorFormat: KSDATAFORMAT_TYPE_AUDIO,
                    SubFormat: pcm,
                    Specifier: KSDATAFORMAT_SPECIFIER_WAVEFORMATEX,
                    ..Default::default()
                },
            },
            wave: WAVEFORMATEXTENSIBLE {
                Format: WAVEFORMATEX {
                    wFormatTag: 0xfffe,
                    nChannels: 2,
                    nSamplesPerSec: 48000,
                    nAvgBytesPerSec: 192000,
                    nBlockAlign: 4,
                    wBitsPerSample: 16,
                    cbSize: 22,
                },
                Samples: WAVEFORMATEXTENSIBLE_0 {
                    wValidBitsPerSample: 16,
                },
                dwChannelMask: 3,
                SubFormat: pcm,
            },
        };
        let event = EventHandle::new()?;
        let mut pin = HANDLE::default();
        let result = KsCreatePin(filter.0, &connection.connect, GENERIC_READ.0, &mut pin);
        if result != 0 {
            bail!("KsCreatePin PCM16 48k stereo: Win32 error {result} (0x{result:08X})");
        }
        let stream = Stream {
            pin: EventHandle(pin),
            event: event.0,
        };
        let request = KSRTAUDIO_BUFFER_PROPERTY_WITH_NOTIFICATION {
            Property: property(
                KSPROPSETID_RtAudio,
                KSPROPERTY_RTAUDIO_BUFFER_WITH_NOTIFICATION.0 as u32,
                KSPROPERTY_TYPE_GET,
            ),
            RequestedBufferSize: options.ks_frames * 4 * 2,
            NotificationCount: 2,
            BaseAddress: std::ptr::null_mut(),
        };
        let mut buffer = KSRTAUDIO_BUFFER::default();
        ioctl(pin, &request, &mut buffer)
            .context("WaveRT buffer allocation (two notifications)")?;
        anyhow::ensure!(
            !buffer.BufferAddress.is_null()
                && buffer.ActualBufferSize >= 8
                && buffer.ActualBufferSize % 8 == 0,
            "invalid WaveRT buffer reply"
        );
        let frames = buffer.ActualBufferSize / 8;
        let request = KSRTAUDIO_NOTIFICATION_EVENT_PROPERTY {
            Property: property(
                KSPROPSETID_RtAudio,
                KSPROPERTY_RTAUDIO_REGISTER_NOTIFICATION_EVENT.0 as u32,
                KSPROPERTY_TYPE_GET,
            ),
            NotificationEvent: event.0,
        };
        ioctl(pin, &request, &mut [] as &mut [u8; 0]).context("register WaveRT event")?;
        let mut task_index = 0;
        let mmcss = AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task_index);
        let mmcss_error = mmcss.as_ref().err().map(ToString::to_string);
        let _mmcss = mmcss.ok().map(Mmcss);
        startup.send(json!({"event":"startup","selected_backend":"direct KS WaveRT loopback","device_id":path,"device_name":path,"ks_pin":pin_id,
            "format":{"sample_rate":48000,"channels":2,"sample_format":"pcm16_le","bits_per_sample":16},
            "requested_period_frames":options.ks_frames,"actual_notification_frames":frames,"capture_buffer_frames":frames*2,"mmcss_error":mmcss_error,
            "tap_mode":"driver loopback pin","pre_volume_pre_mute_verified":false,"duration_requested_seconds":options.seconds}))?;
        state(pin, KSSTATE_ACQUIRE).context("KSSTATE_ACQUIRE")?;
        state(pin, KSSTATE_PAUSE).context("KSSTATE_PAUSE")?;
        let qpc = Qpc::new()?;
        let cpu_start = thread_cpu_100ns()?;
        state(pin, KSSTATE_RUN).context("KSSTATE_RUN")?;
        let start = Instant::now();
        let (mut sequence, mut total, mut wakes, mut not_ready) = (0u64, 0u64, 0u64, 0u64);
        let mut last = None;
        let mut quiet = 0u64;
        let mut position_fallback = false;
        let mut ambiguous_wraps = 0u64;
        let mut previous_cursor = None;
        let mut cursor_gap_risks = 0u64;
        while start.elapsed() < Duration::from_secs(options.seconds)
            && !shared.stop.load(Ordering::Relaxed)
        {
            let wait = WaitForSingleObject(event.0, 250);
            if wait == WAIT_TIMEOUT {
                continue;
            }
            anyhow::ensure!(wait == WAIT_OBJECT_0, "KS notification wait failed");
            let wake = qpc.now();
            if last.is_some_and(|t| wake - t >= frames as u64 * 2 * 10_000_000 / 48000) {
                ambiguous_wraps += 1;
            }
            let (mut wake_frames, mut packets) = (0u64, 0u32);
            loop {
                let begin = qpc.now();
                let mut info = KSRTAUDIO_GETREADPACKET_INFO::default();
                let result = if position_fallback {
                    Ok(())
                } else {
                    ioctl(
                        pin,
                        &property(
                            KSPROPSETID_RtAudio,
                            KSPROPERTY_RTAUDIO_GETREADPACKET.0 as u32,
                            KSPROPERTY_TYPE_GET,
                        ),
                        &mut info,
                    )
                };
                let mut buffer_offset = None;
                let mut read_frames = frames;
                match result {
                    Ok(()) => {}
                    Err(e)
                        if options.ks_position
                            && matches!(
                                e.code().0 as u32,
                                0x80070490 | 0x80070492 | 0x80070032
                            ) =>
                    {
                        position_fallback = true;
                    }
                    Err(e) if e.code().0 == 0x80070015u32 as i32 => {
                        not_ready += 1;
                        break;
                    }
                    Err(e) => return Err(e).context("WaveRT GETREADPACKET"),
                }
                if position_fallback {
                    let mut pos = KSAUDIO_POSITION::default();
                    ioctl(
                        pin,
                        &property(
                            KSPROPSETID_Audio,
                            KSPROPERTY_AUDIO_POSITION.0 as u32,
                            KSPROPERTY_TYPE_GET,
                        ),
                        &mut pos,
                    )
                    .context("KS AUDIO_POSITION fallback")?;
                    anyhow::ensure!(
                        pos.PlayOffset < buffer.ActualBufferSize as u64,
                        "driver cursor outside cyclic buffer"
                    );
                    let position_time = qpc.now();
                    let cursor = ((pos.PlayOffset + 1) / 4 % (frames * 2) as u64) as u32;
                    shared.push(Record::KernelPosition {
                        time_100ns: position_time,
                        play_offset: pos.PlayOffset,
                        write_offset: pos.WriteOffset,
                    });
                    let previous = previous_cursor.replace((cursor, position_time));
                    let Some((old_cursor, old_time)) = previous else {
                        break;
                    };
                    if position_time - old_time >= frames as u64 * 2 * 10_000_000 / 48000 {
                        cursor_gap_risks += 1;
                    }
                    read_frames = cursor_delta(old_cursor, cursor, frames * 2);
                    buffer_offset = Some(old_cursor * 4);
                    if read_frames == 0 {
                        break;
                    }
                }
                let acquired = qpc.now();
                if buffer.CallMemoryBarrier.as_bool() {
                    std::sync::atomic::fence(Ordering::SeqCst);
                }
                let mut peak = 0.0f32;
                let first_sample = if position_fallback {
                    total
                } else {
                    info.PacketNumber as u64 * frames as u64
                };
                if options.measure_level || options.pulse_probe {
                    let offset = buffer_offset.unwrap_or((info.PacketNumber % 2) * frames * 4);
                    let ptr = buffer.BufferAddress.cast::<i16>();
                    for frame in 0..read_frames {
                        let index = ((offset / 4 + frame) % (frames * 2)) as usize * 2;
                        let a = ptr.add(index).read_volatile() as f32 / 32768.0;
                        let b = ptr.add(index + 1).read_volatile() as f32 / 32768.0;
                        let p = a.abs().max(b.abs());
                        peak = peak.max(p);
                        if options.pulse_probe {
                            if p > 0.0001 {
                                if quiet >= 4800 {
                                    shared.push(Record::Pulse {
                                        capture_time_100ns: acquired,
                                        first_sample,
                                        offset_frames: frame,
                                    });
                                }
                                quiet = 0;
                            } else {
                                quiet += 1;
                            }
                        }
                    }
                }
                let timestamp = ((info.PerformanceCounterValue as u128 * 10_000_000)
                    / qpc.frequency as u128) as u64;
                shared.push(Record::Capture {
                    sequence,
                    first_sample,
                    device_position: first_sample,
                    device_position_valid: !position_fallback,
                    frames: read_frames,
                    flags: 0,
                    capture_time_100ns: acquired,
                    first_sample_qpc_100ns: (timestamp != 0).then_some(timestamp),
                    first_sample_age_us: (timestamp != 0)
                        .then(|| ((acquired as i128 - timestamp as i128) / 10) as i64),
                    get_buffer_us: (acquired - begin) / 10,
                    capture_to_record_us: (qpc.now() - acquired) / 10,
                    peak: options.measure_level.then_some(peak),
                });
                sequence += 1;
                total += read_frames as u64;
                wake_frames += read_frames as u64;
                packets += 1;
                if !info.MoreData.as_bool()
                    || shared.stop.load(Ordering::Relaxed)
                    || start.elapsed() >= Duration::from_secs(options.seconds)
                {
                    break;
                }
            }
            shared.push(Record::Wake {
                time_100ns: wake,
                interval_us: last.map(|t| (wake - t) / 10),
                frames: wake_frames,
                packets,
                drain_us: (qpc.now() - wake) / 10,
            });
            last = Some(wake);
            wakes += 1;
        }
        state(pin, KSSTATE_STOP)?;
        let elapsed = start.elapsed().as_secs_f64();
        let cpu = thread_cpu_100ns()?.saturating_sub(cpu_start);
        drop(stream);
        // Keep format/filter/event alive until the pin and its mapped buffer have closed.
        let _ = &mut connection;
        Ok(
            json!({"event":"capture_end","elapsed_seconds":elapsed,"captured_packets":sequence,"captured_frames":total,
            "wake_count":wakes,"driver_not_ready":not_ready,"position_fallback_used":position_fallback,"audio_timeline_valid":!position_fallback,
            "event_gaps_ge_full_buffer_duration":ambiguous_wraps,"capture_thread_cpu_ms":cpu as f64/10000.0,
            "cursor_gaps_ge_full_buffer_duration":cursor_gap_risks,
            "capture_thread_cpu_one_core_percent":cpu as f64/100000.0/elapsed,"telemetry_dropped":shared.dropped.load(Ordering::Relaxed)}),
        )
    }
}

fn cursor_delta(previous: u32, current: u32, ring_frames: u32) -> u32 {
    (current + ring_frames - previous) % ring_frames
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drains_48_frames_per_tick_in_64_frame_ring_instead_of_only_half() {
        assert_eq!(cursor_delta(0, 48, 64), 48);
        assert_eq!(cursor_delta(48, 32, 64), 48);
        assert_eq!(cursor_delta(32, 16, 64), 48);
    }
    #[test]
    fn unchanged_cursor_cannot_distinguish_idle_from_whole_ring_wrap() {
        assert_eq!(cursor_delta(16, 16, 64), 0);
    }
}
