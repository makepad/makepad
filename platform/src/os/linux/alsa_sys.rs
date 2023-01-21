#![allow(non_camel_case_types)]

pub type snd_pcm_info_t = _snd_pcm_info;
pub type _snd_pcm_format = ::std::os::raw::c_int;
pub use self::_snd_pcm_format as snd_pcm_format_t;
pub type _snd_pcm_stream = ::std::os::raw::c_uint;
pub const SND_PCM_FORMAT_FLOAT_LE: _snd_pcm_format = 14;
pub const SND_PCM_STREAM_PLAYBACK: _snd_pcm_stream = 0;
pub const SND_PCM_STREAM_CAPTURE: _snd_pcm_stream = 1;
pub type snd_pcm_hw_params_t = _snd_pcm_hw_params;
pub type snd_pcm_t = _snd_pcm;
pub use self::_snd_pcm_stream as snd_pcm_stream_t;
pub type _snd_pcm_access = ::std::os::raw::c_uint;
pub use self::_snd_pcm_access as snd_pcm_access_t;
pub const SND_PCM_ACCESS_RW_INTERLEAVED: _snd_pcm_access = 3;
pub type snd_pcm_uframes_t = ::std::os::raw::c_ulong;
pub type snd_pcm_sframes_t = ::std::os::raw::c_long;
pub type snd_output_t = _snd_output;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _snd_output {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _snd_pcm {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _snd_ctl {
    _unused: [u8; 0],
}
pub type snd_ctl_t = _snd_ctl;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _snd_pcm_info {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _snd_pcm_hw_params {
    _unused: [u8; 0],
}

#[link(name = "asound")]
extern "C" {
    pub fn snd_card_next(card: *mut ::std::os::raw::c_int) -> ::std::os::raw::c_int;
    
    pub fn snd_strerror(errnum: ::std::os::raw::c_int) -> *const ::std::os::raw::c_char;
    
    pub fn snd_ctl_open(
        ctl: *mut *mut snd_ctl_t,
        name: *const ::std::os::raw::c_char,
        mode: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_ctl_pcm_next_device(
        ctl: *mut snd_ctl_t,
        device: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_device_name_hint(
        card: ::std::os::raw::c_int,
        iface: *const ::std::os::raw::c_char,
        hints: *mut *mut *mut ::std::os::raw::c_void,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_device_name_get_hint(
        hint: *const ::std::os::raw::c_void,
        id: *const ::std::os::raw::c_char,
    ) -> *mut ::std::os::raw::c_char;
    
    pub fn snd_pcm_open(
        pcm: *mut *mut snd_pcm_t,
        name: *const ::std::os::raw::c_char,
        stream: snd_pcm_stream_t,
        mode: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_malloc(ptr: *mut *mut snd_pcm_hw_params_t) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_any(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_set_access(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        _access: snd_pcm_access_t,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_set_format(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: snd_pcm_format_t,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_set_rate_near(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: *mut ::std::os::raw::c_uint,
        dir: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_set_channels(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: ::std::os::raw::c_uint,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_set_rate_resample(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: ::std::os::raw::c_uint,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_get_buffer_size(
        params: *const snd_pcm_hw_params_t,
        val: *mut snd_pcm_uframes_t,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_free(obj: *mut snd_pcm_hw_params_t);
    
    pub fn snd_pcm_prepare(pcm: *mut snd_pcm_t) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_get_channels(
        params: *const snd_pcm_hw_params_t,
        val: *mut ::std::os::raw::c_uint,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_get_rate(
        params: *const snd_pcm_hw_params_t,
        val: *mut ::std::os::raw::c_uint,
        dir: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_get_period_size(
        params: *const snd_pcm_hw_params_t,
        frames: *mut snd_pcm_uframes_t,
        dir: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_hw_params_get_period_time(
        params: *const snd_pcm_hw_params_t,
        val: *mut ::std::os::raw::c_uint,
        dir: *mut ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_pcm_writei(
        pcm: *mut snd_pcm_t,
        buffer: *const ::std::os::raw::c_void,
        size: snd_pcm_uframes_t,
    ) -> snd_pcm_sframes_t;
    pub fn snd_pcm_hw_params_set_periods(
        pcm: *mut snd_pcm_t,
        params: *mut snd_pcm_hw_params_t,
        val: ::std::os::raw::c_uint,
        dir: ::std::os::raw::c_int,
    ) -> ::std::os::raw::c_int;
}

