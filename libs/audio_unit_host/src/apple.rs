//! The macOS side: AudioToolbox's component and Audio Unit C API.

use crate::{ComponentDescription, ComponentInfo, Error, MidiMessage};
use std::ffi::c_void;
use std::ptr;

type OsStatus = i32;
type AudioComponent = *mut c_void;
type AudioUnit = *mut c_void;
type CfStringRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SmpteTime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    smpte_type: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioTimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock_time: u64,
    smpte_time: SmpteTime,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

/// `AudioBufferList` with room for the two buffers of a non-interleaved
/// stereo stream. `repr(C)` pads after the count exactly as C does.
#[repr(C)]
struct StereoBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 2],
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(component: AudioComponent, desc: *const AudioComponentDescription) -> AudioComponent;
    fn AudioComponentCopyName(component: AudioComponent, name: *mut CfStringRef) -> OsStatus;
    fn AudioComponentGetDescription(component: AudioComponent, desc: *mut AudioComponentDescription) -> OsStatus;
    fn AudioComponentGetVersion(component: AudioComponent, version: *mut u32) -> OsStatus;
    fn AudioComponentInstanceNew(component: AudioComponent, instance: *mut AudioUnit) -> OsStatus;
    fn AudioComponentInstanceDispose(instance: AudioUnit) -> OsStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OsStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OsStatus;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> OsStatus;
    fn AudioUnitGetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> OsStatus;
    fn AudioUnitRender(
        unit: AudioUnit,
        action_flags: *mut u32,
        time_stamp: *const AudioTimeStamp,
        output_bus: u32,
        frames: u32,
        data: *mut StereoBufferList,
    ) -> OsStatus;
    fn AudioUnitReset(unit: AudioUnit, scope: u32, element: u32) -> OsStatus;
    fn MusicDeviceMIDIEvent(unit: AudioUnit, status: u32, data1: u32, data2: u32, offset: u32) -> OsStatus;
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioUnitGetPropertyInfo(unit: AudioUnit, id: u32, scope: u32, element: u32, size: *mut u32, writable: *mut u8) -> OsStatus;
    fn AudioUnitGetParameter(unit: AudioUnit, id: u32, scope: u32, element: u32, value: *mut f32) -> OsStatus;
    fn AudioUnitSetParameter(unit: AudioUnit, id: u32, scope: u32, element: u32, value: f32, offset: u32) -> OsStatus;
}

#[repr(C)]
struct AudioUnitParameterInfo {
    name: [u8; 52],
    unit_name: CfStringRef,
    clump_id: u32,
    cf_name_string: CfStringRef,
    unit: u32,
    min_value: f32,
    max_value: f32,
    default_value: f32,
    flags: u32,
}

#[repr(C)]
struct AuPreset {
    number: i32,
    name: CfStringRef,
}

#[repr(C)]
struct ParameterStringFromValue {
    id: u32,
    value: *const f32,
    text: CfStringRef,
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFRetain(object: *const c_void) -> *const c_void;
    fn CFStringGetLength(string: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(string: CfStringRef, buffer: *mut u8, size: isize, encoding: u32) -> u8;
    fn CFRelease(object: *const c_void);
}

const UTF8: u32 = 0x0800_0100;

const SCOPE_GLOBAL: u32 = 0;
const SCOPE_OUTPUT: u32 = 2;
const PROPERTY_STREAM_FORMAT: u32 = 8;
const PROPERTY_LATENCY: u32 = 12;
const PROPERTY_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
const PROPERTY_OFFLINE_RENDER: u32 = 37;
const PROPERTY_PARAMETER_LIST: u32 = 3;
const PROPERTY_PARAMETER_INFO: u32 = 4;
const PROPERTY_FACTORY_PRESETS: u32 = 24;
const PROPERTY_PRESENT_PRESET: u32 = 36;
const PROPERTY_PARAMETER_STRING_FROM_VALUE: u32 = 33;
const PARAMETER_FLAG_HAS_CF_NAME: u32 = 1 << 27;
const PARAMETER_FLAG_CF_NAME_RELEASE: u32 = 1 << 4;

const FORMAT_LINEAR_PCM: u32 = crate::four_cc(b"lpcm");
const FORMAT_FLAG_FLOAT: u32 = 1;
const FORMAT_FLAG_PACKED: u32 = 8;
const FORMAT_FLAG_NON_INTERLEAVED: u32 = 32;
const TIME_STAMP_SAMPLE_TIME_VALID: u32 = 1;

fn check(call: &'static str, status: OsStatus) -> Result<(), Error> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error { call, status })
    }
}

fn to_c(desc: &ComponentDescription) -> AudioComponentDescription {
    AudioComponentDescription {
        component_type: desc.kind,
        component_sub_type: desc.sub_kind,
        component_manufacturer: desc.manufacturer,
        ..Default::default()
    }
}

/// A CFString's text; releases it.
unsafe fn take_cf_string(string: CfStringRef) -> String {
    if string.is_null() {
        return String::new();
    }
    let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(string), UTF8) + 1;
    let mut buffer = vec![0u8; size.max(1) as usize];
    let ok = CFStringGetCString(string, buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0;
    CFRelease(string);
    if !ok {
        return String::new();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

unsafe fn info_of(component: AudioComponent) -> ComponentInfo {
    let mut desc = AudioComponentDescription::default();
    AudioComponentGetDescription(component, &mut desc);
    let mut name: CfStringRef = ptr::null();
    let name = if AudioComponentCopyName(component, &mut name) == 0 { take_cf_string(name) } else { String::new() };
    let mut version = 0;
    AudioComponentGetVersion(component, &mut version);
    ComponentInfo {
        description: ComponentDescription {
            kind: desc.component_type,
            sub_kind: desc.component_sub_type,
            manufacturer: desc.component_manufacturer,
        },
        name,
        version,
    }
}

pub(crate) fn components(desc: &ComponentDescription) -> Vec<ComponentInfo> {
    let c_desc = to_c(desc);
    let mut out = Vec::new();
    let mut component: AudioComponent = ptr::null_mut();
    loop {
        // Safety: a null or previously returned component and a valid
        // description; the call only reads them.
        component = unsafe { AudioComponentFindNext(component, &c_desc) };
        if component.is_null() {
            return out;
        }
        out.push(unsafe { info_of(component) });
    }
}

pub(crate) struct Unit {
    unit: AudioUnit,
    initialized: bool,
    sample_time: f64,
    buffers: StereoBufferList,
}

// An Audio Unit instance may be used from any one thread at a time.
unsafe impl Send for Unit {}

impl Unit {
    pub(crate) fn new(desc: &ComponentDescription, sample_rate: f64, max_frames: u32) -> Result<(Unit, ComponentInfo), Error> {
        let c_desc = to_c(desc);
        // Safety: a valid description; the result is a component or null.
        let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &c_desc) };
        if component.is_null() {
            return Err(Error::not_found());
        }
        let info = unsafe { info_of(component) };
        let mut unit: AudioUnit = ptr::null_mut();
        check("AudioComponentInstanceNew", unsafe { AudioComponentInstanceNew(component, &mut unit) })?;
        if unit.is_null() {
            return Err(Error { call: "AudioComponentInstanceNew", status: -1 });
        }
        let mut out = Unit {
            unit,
            initialized: false,
            sample_time: 0.0,
            buffers: StereoBufferList {
                number_buffers: 2,
                buffers: [AudioBuffer { number_channels: 1, data_byte_size: 0, data: ptr::null_mut() }; 2],
            },
        };
        let format = AudioStreamBasicDescription {
            sample_rate,
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED | FORMAT_FLAG_NON_INTERLEAVED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };
        out.set_property("set stream format", PROPERTY_STREAM_FORMAT, SCOPE_OUTPUT, &format)?;
        out.set_property("set maximum frames per slice", PROPERTY_MAXIMUM_FRAMES_PER_SLICE, SCOPE_GLOBAL, &max_frames)?;
        check("AudioUnitInitialize", unsafe { AudioUnitInitialize(out.unit) })?;
        out.initialized = true;
        Ok((out, info))
    }

    fn set_property<T>(&mut self, call: &'static str, id: u32, scope: u32, value: &T) -> Result<(), Error> {
        // Safety: `value` is a live `T` of exactly the size passed.
        let status = unsafe {
            AudioUnitSetProperty(self.unit, id, scope, 0, value as *const T as *const c_void, std::mem::size_of::<T>() as u32)
        };
        check(call, status)
    }

    pub(crate) fn set_offline(&mut self, offline: bool) -> Result<(), Error> {
        self.set_property("set offline render", PROPERTY_OFFLINE_RENDER, SCOPE_GLOBAL, &u32::from(offline))
    }

    pub(crate) fn latency_seconds(&self) -> f64 {
        let mut value = 0.0f64;
        let mut size = std::mem::size_of::<f64>() as u32;
        // Safety: an f64 out-parameter of the size passed.
        let status = unsafe {
            AudioUnitGetProperty(self.unit, PROPERTY_LATENCY, SCOPE_GLOBAL, 0, &mut value as *mut f64 as *mut c_void, &mut size)
        };
        if status == 0 && value.is_finite() && value > 0.0 {
            value
        } else {
            0.0
        }
    }

    pub(crate) fn midi(&mut self, message: MidiMessage, offset: u32) -> Result<(), Error> {
        // Safety: an initialised unit; plain integers.
        let status = unsafe {
            MusicDeviceMIDIEvent(self.unit, message.status as u32, message.data1 as u32, message.data2 as u32, offset)
        };
        check("MusicDeviceMIDIEvent", status)
    }

    /// One slice of at most the maximum frames per slice.
    pub(crate) fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> Result<(), Error> {
        let frames = left.len().min(right.len());
        let bytes = (frames * std::mem::size_of::<f32>()) as u32;
        let targets = [left.as_mut_ptr(), right.as_mut_ptr()];
        for (buffer, target) in self.buffers.buffers.iter_mut().zip(targets) {
            buffer.number_channels = 1;
            buffer.data_byte_size = bytes;
            buffer.data = target as *mut c_void;
        }
        self.buffers.number_buffers = 2;
        let stamp = AudioTimeStamp { sample_time: self.sample_time, flags: TIME_STAMP_SAMPLE_TIME_VALID, ..Default::default() };
        let mut flags = 0u32;
        // Safety: both buffers point at `frames` writable floats that outlive
        // the call; the unit was initialised for this format.
        let status = unsafe { AudioUnitRender(self.unit, &mut flags, &stamp, 0, frames as u32, &mut self.buffers) };
        check("AudioUnitRender", status)?;
        // A unit may answer with buffers of its own; copy from those.
        for (buffer, target) in self.buffers.buffers.iter().zip([left, right]) {
            let data = buffer.data as *const f32;
            if !data.is_null() && data != target.as_ptr() {
                // Safety: the unit's buffer holds the frames just rendered.
                let source = unsafe { std::slice::from_raw_parts(data, frames) };
                target[..frames].copy_from_slice(source);
            }
        }
        self.sample_time += frames as f64;
        Ok(())
    }

    pub(crate) fn parameters(&self) -> Vec<crate::ParameterInfo> {
        let mut size = 0u32;
        let mut writable = 0u8;
        // Safety: out-parameters only.
        let status = unsafe { AudioUnitGetPropertyInfo(self.unit, PROPERTY_PARAMETER_LIST, SCOPE_GLOBAL, 0, &mut size, &mut writable) };
        if status != 0 || size == 0 {
            return Vec::new();
        }
        let mut ids = vec![0u32; size as usize / 4];
        // Safety: `ids` holds `size` bytes.
        let status = unsafe {
            AudioUnitGetProperty(self.unit, PROPERTY_PARAMETER_LIST, SCOPE_GLOBAL, 0, ids.as_mut_ptr() as *mut c_void, &mut size)
        };
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / 4);
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let mut info = AudioUnitParameterInfo {
                name: [0; 52],
                unit_name: ptr::null(),
                clump_id: 0,
                cf_name_string: ptr::null(),
                unit: 0,
                min_value: 0.0,
                max_value: 0.0,
                default_value: 0.0,
                flags: 0,
            };
            let mut size = std::mem::size_of::<AudioUnitParameterInfo>() as u32;
            // Safety: an info struct of the size passed; the parameter id is the element.
            let status = unsafe {
                AudioUnitGetProperty(self.unit, PROPERTY_PARAMETER_INFO, SCOPE_GLOBAL, id, &mut info as *mut _ as *mut c_void, &mut size)
            };
            if status != 0 {
                continue;
            }
            let name = if info.flags & PARAMETER_FLAG_HAS_CF_NAME != 0 && !info.cf_name_string.is_null() {
                // Safety: the unit handed us this string; we own a reference
                // only when it says so, so take one otherwise.
                unsafe {
                    if info.flags & PARAMETER_FLAG_CF_NAME_RELEASE == 0 {
                        CFRetain(info.cf_name_string);
                    }
                    take_cf_string(info.cf_name_string)
                }
            } else {
                let end = info.name.iter().position(|&b| b == 0).unwrap_or(info.name.len());
                String::from_utf8_lossy(&info.name[..end]).into_owned()
            };
            out.push(crate::ParameterInfo {
                id,
                name,
                min: info.min_value,
                max: info.max_value,
                default: info.default_value,
            });
        }
        out
    }

    pub(crate) fn parameter(&self, id: u32) -> Result<f32, Error> {
        let mut value = 0.0f32;
        check("AudioUnitGetParameter", unsafe { AudioUnitGetParameter(self.unit, id, SCOPE_GLOBAL, 0, &mut value) })?;
        Ok(value)
    }

    pub(crate) fn set_parameter(&mut self, id: u32, value: f32) -> Result<(), Error> {
        check("AudioUnitSetParameter", unsafe { AudioUnitSetParameter(self.unit, id, SCOPE_GLOBAL, 0, value, 0) })
    }

    pub(crate) fn parameter_text(&self, id: u32, value: f32) -> Option<String> {
        let mut query = ParameterStringFromValue { id, value: &value, text: ptr::null() };
        let mut size = std::mem::size_of::<ParameterStringFromValue>() as u32;
        // Safety: the query struct of the size passed; the unit fills `text`
        // with a string the caller releases.
        let status = unsafe {
            AudioUnitGetProperty(
                self.unit,
                PROPERTY_PARAMETER_STRING_FROM_VALUE,
                SCOPE_GLOBAL,
                0,
                &mut query as *mut _ as *mut c_void,
                &mut size,
            )
        };
        (status == 0 && !query.text.is_null()).then(|| unsafe { take_cf_string(query.text) })
    }

    pub(crate) fn factory_presets(&self) -> Vec<(i32, String)> {
        let mut array: *const c_void = ptr::null();
        let mut size = std::mem::size_of::<*const c_void>() as u32;
        // Safety: a CFArray out-parameter; we release the array we are given.
        let status = unsafe {
            AudioUnitGetProperty(self.unit, PROPERTY_FACTORY_PRESETS, SCOPE_GLOBAL, 0, &mut array as *mut _ as *mut c_void, &mut size)
        };
        if status != 0 || array.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        unsafe {
            for index in 0..CFArrayGetCount(array) {
                let preset = CFArrayGetValueAtIndex(array, index) as *const AuPreset;
                if preset.is_null() {
                    continue;
                }
                let name = (*preset).name;
                let text = if name.is_null() {
                    String::new()
                } else {
                    // The array owns the name: take a reference of our own.
                    CFRetain(name);
                    take_cf_string(name)
                };
                out.push(((*preset).number, text));
            }
            CFRelease(array);
        }
        out
    }

    pub(crate) fn set_factory_preset(&mut self, number: i32) -> Result<(), Error> {
        let preset = AuPreset { number, name: ptr::null() };
        self.set_property("set present preset", PROPERTY_PRESENT_PRESET, SCOPE_GLOBAL, &preset)
    }

    pub(crate) fn reset(&mut self) -> Result<(), Error> {
        check("AudioUnitReset", unsafe { AudioUnitReset(self.unit, SCOPE_GLOBAL, 0) })
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        // Safety: the unit is ours and nothing renders it any more.
        unsafe {
            if self.initialized {
                AudioUnitUninitialize(self.unit);
            }
            AudioComponentInstanceDispose(self.unit);
        }
    }
}
