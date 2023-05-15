#![allow(non_camel_case_types)]

use std::os::raw::c_void;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AAudioStreamBuilderStruct {
    _unused: [u8; 0],
}
pub type AAudioStreamBuilder = AAudioStreamBuilderStruct;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AAudioStreamStruct {
    _unused: [u8; 0],
}
pub type AAudioStream = AAudioStreamStruct;

pub type aaudio_result_t = i32;
pub type aaudio_direction_t = i32;
pub type aaudio_sharing_mode_t = i32;
pub type aaudio_format_t = i32;
pub type aaudio_stream_state_t = i32;
pub type aaudio_data_callback_result_t = i32;
pub type aaudio_performance_mode_t = i32;

pub type AAudioStream_dataCallback = ::std::option::Option<
unsafe extern "C" fn(
    stream: *mut AAudioStream,
    userData: *mut c_void,
    audioData: *mut c_void,
    numFrames: i32,
) -> aaudio_data_callback_result_t,
>;

pub type AAudioStream_errorCallback = ::std::option::Option<
unsafe extern "C" fn(
    stream: *mut AAudioStream,
    userData: *mut ::std::os::raw::c_void,
    error: aaudio_result_t,
),
>;

#[link(name = "aaudio")]
extern "C" {
    pub fn AAudio_createStreamBuilder(builder: *mut *mut AAudioStreamBuilder) -> aaudio_result_t;
    pub fn AAudioStreamBuilder_setDeviceId(builder: *mut AAudioStreamBuilder, deviceId: i32);
    pub fn AAudioStreamBuilder_setDirection(
        builder: *mut AAudioStreamBuilder,
        direction: aaudio_direction_t,
    );
    pub fn AAudioStreamBuilder_setSharingMode(
        builder: *mut AAudioStreamBuilder,
        sharingMode: aaudio_sharing_mode_t,
    );
    pub fn AAudioStreamBuilder_setSampleRate(builder: *mut AAudioStreamBuilder, sampleRate: i32);
    pub fn AAudioStreamBuilder_setChannelCount(
        builder: *mut AAudioStreamBuilder,
        channelCount: i32,
    );
    pub fn AAudioStreamBuilder_setFormat(
        builder: *mut AAudioStreamBuilder,
        format: aaudio_format_t,
    );
    pub fn AAudioStreamBuilder_setBufferCapacityInFrames(
        builder: *mut AAudioStreamBuilder,
        numFrames: i32,
    );
    pub fn AAudioStreamBuilder_openStream(
        builder: *mut AAudioStreamBuilder,
        stream: *mut *mut AAudioStream,
    ) -> aaudio_result_t;
    
    pub fn AAudioStream_requestStart(stream: *mut AAudioStream) -> aaudio_result_t;
    pub fn AAudioStream_close(stream: *mut AAudioStream) -> aaudio_result_t;
    pub fn AAudioStream_requestStop(stream: *mut AAudioStream) -> aaudio_result_t;
    pub fn AAudioStreamBuilder_delete(builder: *mut AAudioStreamBuilder) -> aaudio_result_t;
    pub fn AAudioStream_waitForStateChange(
        stream: *mut AAudioStream,
        inputState: aaudio_stream_state_t,
        nextState: *mut aaudio_stream_state_t,
        timeoutNanoseconds: i64,
    ) -> aaudio_result_t;
    
    pub fn AAudioStreamBuilder_setDataCallback(
        builder: *mut AAudioStreamBuilder,
        callback: AAudioStream_dataCallback,
        userData: *mut c_void,
    );
    pub fn AAudioStreamBuilder_setPerformanceMode(
        builder: *mut AAudioStreamBuilder,
        mode: aaudio_performance_mode_t,
    );
    pub fn AAudioStream_getChannelCount(stream: *mut AAudioStream) -> i32;
    pub fn AAudioStream_getSampleRate(stream: *mut AAudioStream) -> i32;
    pub fn AAudioStream_getFormat(stream: *mut AAudioStream) -> aaudio_format_t;
    pub fn AAudioStreamBuilder_setErrorCallback(
        builder: *mut AAudioStreamBuilder,
        callback: AAudioStream_errorCallback,
        userData: *mut ::std::os::raw::c_void,
    );

}

pub const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: aaudio_performance_mode_t = 12;

pub const AAUDIO_STREAM_STATE_UNINITIALIZED: aaudio_stream_state_t = 0;
pub const AAUDIO_STREAM_STATE_STOPPING: aaudio_stream_state_t = 9;

pub const AAUDIO_FORMAT_PCM_FLOAT: aaudio_format_t = 2;

pub const AAUDIO_SHARING_MODE_EXCLUSIVE: aaudio_sharing_mode_t = 0;
pub const AAUDIO_SHARING_MODE_SHARED: aaudio_sharing_mode_t = 1;

pub const AAUDIO_DIRECTION_OUTPUT: aaudio_direction_t = 0;
pub const AAUDIO_DIRECTION_INPUT: aaudio_direction_t = 1;

pub const AAUDIO_CALLBACK_RESULT_CONTINUE: aaudio_data_callback_result_t = 0;

pub const AAUDIO_TYPE_UNKNOWN: u32 = 0;
pub const AAUDIO_TYPE_BUILTIN_EARPIECE: u32 = 1;
pub const AAUDIO_TYPE_BUILTIN_SPEAKER: u32 = 2;
pub const AAUDIO_TYPE_WIRED_HEADSET: u32 = 3;
pub const AAUDIO_TYPE_WIRED_HEADPHONES: u32 = 4;
pub const AAUDIO_TYPE_LINE_ANALOG: u32 = 5;
pub const AAUDIO_TYPE_LINE_DIGITAL: u32 = 6;
pub const AAUDIO_TYPE_BLUETOOTH_SCO: u32 = 7;
pub const AAUDIO_TYPE_BLUETOOTH_A2DP: u32 = 8;
pub const AAUDIO_TYPE_HDMI: u32 = 9;
pub const AAUDIO_TYPE_HDMI_ARC: u32 = 10;
pub const AAUDIO_TYPE_USB_DEVICE: u32 = 11;
pub const AAUDIO_TYPE_USB_ACCESSORY: u32 = 12;
pub const AAUDIO_TYPE_DOCK: u32 = 13;
pub const AAUDIO_TYPE_FM: u32 = 14;
pub const AAUDIO_TYPE_BUILTIN_MIC: u32 = 15;
pub const AAUDIO_TYPE_FM_TUNER: u32 = 16;
pub const AAUDIO_TYPE_TV_TUNER: u32 = 17;
pub const AAUDIO_TYPE_TELEPHONY: u32 = 18;
pub const AAUDIO_TYPE_AUX_LINE: u32 = 19;
pub const AAUDIO_TYPE_IP: u32 = 20;
pub const AAUDIO_TYPE_BUS: u32 = 21;
pub const AAUDIO_TYPE_USB_HEADSET: u32 = 22;
pub const AAUDIO_TYPE_HEARING_AID: u32 = 23;
pub const AAUDIO_TYPE_BUILTIN_SPEAKER_SAFE: u32 = 24;
pub const AAUDIO_TYPE_REMOTE_SUBMIX: u32 = 25;
pub const AAUDIO_TYPE_BLE_HEADSET: u32 = 26;
pub const AAUDIO_TYPE_BLE_SPEAKER: u32 = 27;
pub const AAUDIO_TYPE_ECHO_REFERENCE: u32 = 28;
pub const AAUDIO_TYPE_HDMI_EARC: u32 = 29;
pub const AAUDIO_TYPE_BLE_BROADCAST: u32 = 30;

pub const AAUDIO_OK: aaudio_result_t = 0;
pub const AAUDIO_ERROR_BASE: aaudio_result_t = -900;
pub const AAUDIO_ERROR_DISCONNECTED: aaudio_result_t = -899;
pub const AAUDIO_ERROR_ILLEGAL_ARGUMENT: aaudio_result_t = -898;
pub const AAUDIO_ERROR_INTERNAL: aaudio_result_t = -896;
pub const AAUDIO_ERROR_INVALID_STATE: aaudio_result_t = -895;
pub const AAUDIO_ERROR_INVALID_HANDLE: aaudio_result_t = -892;
pub const AAUDIO_ERROR_UNIMPLEMENTED: aaudio_result_t = -890;
pub const AAUDIO_ERROR_UNAVAILABLE: aaudio_result_t = -889;
pub const AAUDIO_ERROR_NO_FREE_HANDLES: aaudio_result_t = -888;
pub const AAUDIO_ERROR_NO_MEMORY: aaudio_result_t = -887;
pub const AAUDIO_ERROR_NULL: aaudio_result_t = -886;
pub const AAUDIO_ERROR_TIMEOUT: aaudio_result_t = -885;
pub const AAUDIO_ERROR_WOULD_BLOCK: aaudio_result_t = -884;
pub const AAUDIO_ERROR_INVALID_FORMAT: aaudio_result_t = -883;
pub const AAUDIO_ERROR_OUT_OF_RANGE: aaudio_result_t = -882;
pub const AAUDIO_ERROR_NO_SERVICE: aaudio_result_t = -881;
pub const AAUDIO_ERROR_INVALID_RATE: aaudio_result_t = -880;
