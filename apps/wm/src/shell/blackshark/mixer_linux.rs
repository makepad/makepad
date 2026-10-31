//! The headset microphone's hardware gain: the receiver's own USB audio
//! feature unit, exposed by snd-usb-audio as the ALSA simple control "Mic"
//! (Mic Capture Volume) on the receiver's sound card. Read and written in
//! hundredths of a dB through libasound's dB mapping of the control's TLV,
//! so the range and the steps are whatever the hardware reports. The
//! playback (PCM) controls are never touched.
//!
//! The card is found from the receiver's USB device each time, because its
//! index changes with plugging order. Called on the system worker only.

use super::MicGain;
use std::{
    ffi::{c_char, c_int, c_long, c_uint, c_void, CStr, CString},
    fs,
    path::Path,
};

const SND_MIXER_SCHN_MONO: c_int = 0;
/// Round a requested dB value to the nearest hardware step.
const DIR_NEAREST: c_int = 0;
const ELEMENT: &str = "Mic";

#[repr(C)]
struct SndMixer {
    _unused: [u8; 0],
}

#[repr(C)]
struct SndMixerElem {
    _unused: [u8; 0],
}

// libasound's own names (`..._dB_...`).
#[allow(non_snake_case)]
#[link(name = "asound")]
extern "C" {
    fn snd_mixer_open(mixer: *mut *mut SndMixer, mode: c_int) -> c_int;
    fn snd_mixer_attach(mixer: *mut SndMixer, name: *const c_char) -> c_int;
    fn snd_mixer_selem_register(mixer: *mut SndMixer, options: *mut c_void, classp: *mut *mut c_void) -> c_int;
    fn snd_mixer_load(mixer: *mut SndMixer) -> c_int;
    fn snd_mixer_close(mixer: *mut SndMixer) -> c_int;
    fn snd_mixer_first_elem(mixer: *mut SndMixer) -> *mut SndMixerElem;
    fn snd_mixer_elem_next(elem: *mut SndMixerElem) -> *mut SndMixerElem;
    fn snd_mixer_selem_get_name(elem: *mut SndMixerElem) -> *const c_char;
    fn snd_mixer_selem_get_index(elem: *mut SndMixerElem) -> c_uint;
    fn snd_mixer_selem_has_capture_volume(elem: *mut SndMixerElem) -> c_int;
    fn snd_mixer_selem_get_capture_dB_range(elem: *mut SndMixerElem, min: *mut c_long, max: *mut c_long) -> c_int;
    fn snd_mixer_selem_get_capture_dB(elem: *mut SndMixerElem, channel: c_int, value: *mut c_long) -> c_int;
    fn snd_mixer_selem_set_capture_dB_all(elem: *mut SndMixerElem, value: c_long, dir: c_int) -> c_int;
    fn snd_strerror(errnum: c_int) -> *const c_char;
}

fn alsa_error(what: &str, code: c_int) -> String {
    // SAFETY: snd_strerror returns a static string for any code.
    let text = unsafe { CStr::from_ptr(snd_strerror(code)) };
    format!("{what}: {}", text.to_string_lossy())
}

/// The ALSA card index of the receiver whose USB device is `usb_device`
/// (a canonical `/sys/devices/...` path).
pub fn find_card(usb_device: &Path) -> Option<i32> {
    let entries = fs::read_dir("/sys/class/sound").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(index) = name
            .to_str()
            .and_then(|n| n.strip_prefix("card"))
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        // `cardN/device` is the USB audio interface; its parent is the device.
        let Ok(interface) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        if interface.parent() == Some(usb_device) {
            return Some(index);
        }
    }
    None
}

/// An open mixer on one card, closed on drop.
struct Mixer(*mut SndMixer);

impl Mixer {
    fn open(card: i32) -> Result<Self, String> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: out-pointer to a local; the handle is owned by `Mixer`.
        let rc = unsafe { snd_mixer_open(&mut handle, 0) };
        if rc < 0 {
            return Err(alsa_error("mixer open", rc));
        }
        let mixer = Self(handle);
        let name = CString::new(format!("hw:{card}")).map_err(|_| "bad card name".to_string())?;
        // SAFETY: `mixer.0` is a live handle and `name` outlives the calls.
        unsafe {
            let rc = snd_mixer_attach(mixer.0, name.as_ptr());
            if rc < 0 {
                return Err(alsa_error("mixer attach", rc));
            }
            let rc = snd_mixer_selem_register(mixer.0, std::ptr::null_mut(), std::ptr::null_mut());
            if rc < 0 {
                return Err(alsa_error("mixer register", rc));
            }
            let rc = snd_mixer_load(mixer.0);
            if rc < 0 {
                return Err(alsa_error("mixer load", rc));
            }
        }
        Ok(mixer)
    }

    /// The first "Mic" simple control with a capture volume.
    fn microphone(&self) -> Result<*mut SndMixerElem, String> {
        // SAFETY: elements belong to the live mixer and are only walked here.
        unsafe {
            let mut elem = snd_mixer_first_elem(self.0);
            while !elem.is_null() {
                let name = CStr::from_ptr(snd_mixer_selem_get_name(elem));
                if name.to_bytes() == ELEMENT.as_bytes()
                    && snd_mixer_selem_get_index(elem) == 0
                    && snd_mixer_selem_has_capture_volume(elem) != 0
                {
                    return Ok(elem);
                }
                elem = snd_mixer_elem_next(elem);
            }
        }
        Err("no Mic capture control on the receiver".into())
    }

    fn gain(&self, elem: *mut SndMixerElem) -> Result<MicGain, String> {
        let (mut min, mut max, mut value): (c_long, c_long, c_long) = (0, 0, 0);
        // SAFETY: `elem` belongs to this live mixer; out-pointers are locals.
        unsafe {
            let rc = snd_mixer_selem_get_capture_dB_range(elem, &mut min, &mut max);
            if rc < 0 {
                return Err(alsa_error("Mic dB range", rc));
            }
            let rc = snd_mixer_selem_get_capture_dB(elem, SND_MIXER_SCHN_MONO, &mut value);
            if rc < 0 {
                return Err(alsa_error("Mic dB", rc));
            }
        }
        if min >= max {
            return Err("the Mic control reports no dB range".into());
        }
        Ok(MicGain {
            db100: value as i64,
            min_db100: min as i64,
            max_db100: max as i64,
        })
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        // SAFETY: the handle came from snd_mixer_open and is closed once.
        unsafe {
            snd_mixer_close(self.0);
        }
    }
}

pub fn read(card: i32) -> Result<MicGain, String> {
    let mixer = Mixer::open(card)?;
    let elem = mixer.microphone()?;
    mixer.gain(elem)
}

/// Set the gain to the hardware step nearest `db100` (clamped into the
/// control's own range) and return what the card then reports, read
/// through a fresh mixer so it is the driver's value, not the cache.
pub fn write(card: i32, db100: i64) -> Result<MicGain, String> {
    {
        let mixer = Mixer::open(card)?;
        let elem = mixer.microphone()?;
        let range = mixer.gain(elem)?;
        let target = db100.clamp(range.min_db100, range.max_db100);
        // SAFETY: `elem` belongs to this live mixer.
        let rc = unsafe { snd_mixer_selem_set_capture_dB_all(elem, target as c_long, DIR_NEAREST) };
        if rc < 0 {
            return Err(alsa_error("Mic gain", rc));
        }
    }
    read(card)
}
