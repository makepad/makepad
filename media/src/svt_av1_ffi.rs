use std::ffi::c_void;

#[repr(C)]
pub struct MpSvtAv1Packet {
    pub data: *mut u8,
    pub len: u32,
    pub pts: i64,
    pub flags: u32,
    pub pic_type: u32,
    pub out_buffer: *mut c_void,
}

unsafe extern "C" {
    pub fn mp_svt_av1_encoder_create(
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        bitrate: u32,
        keyint: i32,
        enc_mode: i32,
    ) -> *mut c_void;

    pub fn mp_svt_av1_encoder_send_i420(
        encoder: *mut c_void,
        y: *const u8,
        y_stride: u32,
        u: *const u8,
        u_stride: u32,
        v: *const u8,
        v_stride: u32,
        height: u32,
        pts: i64,
    ) -> i32;

    pub fn mp_svt_av1_encoder_send_eos(encoder: *mut c_void) -> i32;

    pub fn mp_svt_av1_encoder_get_packet_copy(
        encoder: *mut c_void,
        pic_send_done: i32,
        out_packet: *mut MpSvtAv1Packet,
    ) -> i32;

    pub fn mp_svt_av1_packet_free(packet: *mut MpSvtAv1Packet);

    pub fn mp_svt_av1_encoder_destroy(encoder: *mut c_void);
}
