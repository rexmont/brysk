//! Core Audio output for the preview player, the macOS counterpart of the WASAPI sink.
//!
//! A default output AudioUnit pulls interleaved float frames from a small queue; the player
//! writes into that queue and uses the unit's rendered-frame counter as its clock. The device
//! is opened at 48 kHz stereo; FilmCraft resamples the media to that (the player asks for the
//! sink's rate when it reads audio). Any failure (no output device, a busy device) is returned
//! to the caller, which falls back to the silent, clocked `NullSink`.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};

use super::AudioSink;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// 200 ms of audio, the same margin the WASAPI sink keeps.
const CAPACITY_FRAMES: usize = RATE as usize / 5;

type OSStatus = i32;
type AudioUnit = *mut c_void;
type AudioComponent = *mut c_void;
type AURenderCallback = extern "C" fn(*mut c_void, *mut u32, *const c_void, u32, u32, *mut AudioBufferList) -> OSStatus;

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioStreamBasicDescription {
    m_sample_rate: f64,
    m_format_id: u32,
    m_format_flags: u32,
    m_bytes_per_packet: u32,
    m_frames_per_packet: u32,
    m_bytes_per_frame: u32,
    m_channels_per_frame: u32,
    m_bits_per_channel: u32,
    m_reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    m_number_channels: u32,
    m_data_byte_size: u32,
    m_data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    m_number_buffers: u32,
    m_buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct AURenderCallbackStruct {
    input_proc: AURenderCallback,
    input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(component: AudioComponent, desc: *const AudioComponentDescription) -> AudioComponent;
    fn AudioComponentInstanceNew(component: AudioComponent, instance: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(instance: AudioUnit) -> OSStatus;
    fn AudioUnitSetProperty(unit: AudioUnit, property: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> OSStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
}

const K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK: u32 = 23;
const K_AUDIO_UNIT_SCOPE_INPUT: u32 = 1;
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1 << 0;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;

struct Shared {
    queue: Mutex<VecDeque<f32>>,
    played: AtomicU64,
    volume: AtomicU32,
    channels: usize,
}

struct Sink {
    unit: AudioUnit,
    shared: Arc<Shared>,
    paused: AtomicBool,
}

// The unit is only called from its own thread after `AudioOutputUnitStart`; the sink itself is
// used from the player's clock thread.
unsafe impl Send for Sink {}

impl Sink {
    fn open() -> Result<Sink> {
        unsafe {
            let desc = AudioComponentDescription {
                component_type: u32::from_be_bytes(*b"auou"),
                component_sub_type: u32::from_be_bytes(*b"def "),
                component_manufacturer: u32::from_be_bytes(*b"appl"),
                component_flags: 0,
                component_mask: 0,
            };
            let component = AudioComponentFindNext(std::ptr::null_mut(), &desc);
            anyhow::ensure!(!component.is_null(), "no default audio output");
            let mut unit: AudioUnit = std::ptr::null_mut();
            let status = AudioComponentInstanceNew(component, &mut unit);
            anyhow::ensure!(status == 0 && !unit.is_null(), "audio output unit ({status})");

            let format = AudioStreamBasicDescription {
                m_sample_rate: RATE as f64,
                m_format_id: K_AUDIO_FORMAT_LINEAR_PCM,
                m_format_flags: K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_PACKED,
                m_bytes_per_packet: 4 * CHANNELS as u32,
                m_frames_per_packet: 1,
                m_bytes_per_frame: 4 * CHANNELS as u32,
                m_channels_per_frame: CHANNELS as u32,
                m_bits_per_channel: 32,
                m_reserved: 0,
            };
            let status = AudioUnitSetProperty(
                unit,
                K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                K_AUDIO_UNIT_SCOPE_INPUT,
                0,
                &format as *const _ as *const c_void,
                std::mem::size_of::<AudioStreamBasicDescription>() as u32,
            );
            if status != 0 {
                AudioComponentInstanceDispose(unit);
                return Err(anyhow!("audio stream format ({status})"));
            }

            let shared = Arc::new(Shared {
                queue: Mutex::new(VecDeque::with_capacity(CAPACITY_FRAMES * CHANNELS as usize)),
                played: AtomicU64::new(0),
                volume: AtomicU32::new(1.0f32.to_bits()),
                channels: CHANNELS as usize,
            });
            let callback = AURenderCallbackStruct {
                input_proc: render,
                input_proc_ref_con: Arc::as_ptr(&shared) as *mut c_void,
            };
            let status = AudioUnitSetProperty(
                unit,
                K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                K_AUDIO_UNIT_SCOPE_INPUT,
                0,
                &callback as *const _ as *const c_void,
                std::mem::size_of::<AURenderCallbackStruct>() as u32,
            );
            if status != 0 {
                AudioComponentInstanceDispose(unit);
                return Err(anyhow!("audio render callback ({status})"));
            }
            let status = AudioUnitInitialize(unit);
            if status != 0 {
                AudioComponentInstanceDispose(unit);
                return Err(anyhow!("audio unit initialize ({status})"));
            }
            // Start paused; the player calls `pause(false)` when playback begins.
            let sink = Sink { unit, shared, paused: AtomicBool::new(true) };
            Ok(sink)
        }
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        unsafe {
            let _ = AudioOutputUnitStop(self.unit);
            let _ = AudioUnitUninitialize(self.unit);
            let _ = AudioComponentInstanceDispose(self.unit);
        }
    }
}

extern "C" fn render(
    ref_con: *mut c_void,
    _flags: *mut u32,
    _timestamp: *const c_void,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OSStatus {
    if ref_con.is_null() || data.is_null() {
        return 0;
    }
    let shared = unsafe { &*(ref_con as *const Shared) };
    let list = unsafe { &mut *data };
    if list.m_number_buffers == 0 {
        return 0;
    }
    let buffer = &mut list.m_buffers[0];
    let out = buffer.m_data as *mut f32;
    let channels = shared.channels;
    let want = frames as usize * channels;
    if out.is_null() {
        return 0;
    }
    let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
    let mut made = 0usize;
    {
        let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        while made < want {
            match q.pop_front() {
                Some(s) => {
                    unsafe { out.add(made).write(s * volume) };
                    made += 1;
                }
                None => break,
            }
        }
    }
    for i in made..want {
        unsafe { out.add(i).write(0.0) };
    }
    shared.played.fetch_add(frames as u64, Ordering::Relaxed);
    0
}

impl AudioSink for Sink {
    fn sample_rate(&self) -> u32 {
        RATE
    }

    fn channels(&self) -> u16 {
        CHANNELS
    }

    fn write(&mut self, interleaved: &[f32]) -> usize {
        let channels = CHANNELS as usize;
        let frames = interleaved.len() / channels;
        if frames == 0 {
            return 0;
        }
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        let queued = q.len() / channels;
        let room = CAPACITY_FRAMES.saturating_sub(queued);
        let take = frames.min(room);
        if take > 0 {
            q.extend(interleaved[..take * channels].iter().copied());
        }
        take
    }

    fn played_frames(&self) -> u64 {
        self.shared.played.load(Ordering::Relaxed)
    }

    fn pause(&mut self, on: bool) {
        if self.paused.swap(on, Ordering::Relaxed) == on {
            return;
        }
        unsafe {
            if on {
                let _ = AudioOutputUnitStop(self.unit);
            } else {
                let _ = AudioOutputUnitStart(self.unit);
            }
        }
    }

    fn set_volume(&mut self, v: f32) {
        self.shared.volume.store(v.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }

    fn flush(&mut self) {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// The default output device, opened at 48 kHz stereo.
pub fn sink() -> Result<Box<dyn AudioSink>> {
    let s = Sink::open()?;
    Ok(Box::new(s))
}
