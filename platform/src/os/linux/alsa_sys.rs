#![allow(non_camel_case_types)]

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
pub type snd_pcm_info_t = _snd_pcm_info;



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
    
    pub fn snd_card_get_name(
        card: ::std::os::raw::c_int,
        name: *mut *mut ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
    
    pub fn snd_card_get_longname(
        card: ::std::os::raw::c_int,
        name: *mut *mut ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
}
