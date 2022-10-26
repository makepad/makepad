// stripped mac core foundation + metal layer only whats needed

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

pub use {
    std::{
        ffi::c_void,
        os::raw::c_ulong,
        ptr::NonNull,
    },
    
    crate::{
        makepad_platform::{
            os::apple::frameworks::*,
            makepad_objc_sys::{
                runtime::{Class, Object, Protocol, Sel, BOOL, YES, NO},
                declare::ClassDecl,
                msg_send,
                sel,
                class,
                sel_impl,
                Encode,
                Encoding
            },
        },
    }
};


// CORE AUDIO


pub const kAudioUnitManufacturer_Apple: u32 = 1634758764;

#[repr(C)] pub struct OpaqueAudioComponent([u8; 0]);
pub type CAudioComponent = *mut OpaqueAudioComponent;

#[repr(C)] pub struct ComponentInstanceRecord([u8; 0]);
pub type CAudioComponentInstance = *mut ComponentInstanceRecord;
pub type CAudioUnit = CAudioComponentInstance;

pub type OSStatus = i32;

#[repr(C)]
pub struct CAudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: AudioFormatId,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

#[repr(u32)]
pub enum AudioFormatId {
    LinearPCM = 1819304813,
    AC3 = 1633889587,
    F60958AC3 = 1667326771,
    AppleIMA4 = 1768775988,
    MPEG4AAC = 1633772320,
    MPEG4CELP = 1667591280,
    MPEG4HVXC = 1752594531,
    MPEG4TwinVQ = 1953986161,
    MACE3 = 1296122675,
    MACE6 = 1296122678,
    ULaw = 1970037111,
    ALaw = 1634492791,
    QDesign = 1363430723,
    QDesign2 = 1363430706,
    QUALCOMM = 1365470320,
    MPEGLayer1 = 778924081,
    MPEGLayer2 = 778924082,
    MPEGLayer3 = 778924083,
    TimeCode = 1953066341,
    MIDIStream = 1835623529,
    ParameterValueStream = 1634760307,
    AppleLossless = 1634492771,
    MPEG4AAC_HE = 1633772392,
    MPEG4AAC_LD = 1633772396,
    MPEG4AAC_ELD = 1633772389,
    MPEG4AAC_ELD_SBR = 1633772390,
    MPEG4AAC_ELD_V2 = 1633772391,
    MPEG4AAC_HE_V2 = 1633772400,
    MPEG4AAC_Spatial = 1633772403,
    AMR = 1935764850,
    AMR_WB = 1935767394,
    Audible = 1096107074,
    iLBC = 1768710755,
    DVIIntelIMA = 1836253201,
    MicrosoftGSM = 1836253233,
    AES3 = 1634038579,
}
/*
struct F60958AC3Flags;
impl F60958AC3Flags {
    const IS_FLOAT: u32 = 1;
    const IS_BIG_ENDIAN: u32 = 2;
    const IS_SIGNED_INTEGER: u32 = 4;
    const IS_PACKED: u32 = 8;
    const IS_ALIGNED_HIGH: u32 = 16;
    const IS_NON_INTERLEAVED: u32 = 32;
    const IS_NON_MIXABLE: u32 = 64;
}
*/
/*
pub struct LinearPcmFlags;
impl LinearPcmFlags {
    const IS_FLOAT: u32 = 1;
    const IS_BIG_ENDIAN: u32 = 2;
    const IS_SIGNED_INTEGER: u32 = 4;
    const IS_PACKED: u32 = 8;
    const IS_ALIGNED_HIGH: u32 = 16;
    const IS_NON_INTERLEAVED: u32 = 32;
    const IS_NON_MIXABLE: u32 = 64;
    const FLAGS_SAMPLE_FRACTION_SHIFT: u32 = 7;
    const FLAGS_SAMPLE_FRACTION_MASK: u32 = 8064;
}

pub struct AppleLosslessFlags;
impl AppleLosslessFlags {
    const BIT_16_SOURCE_DATA: u32 = 1;
    const BIT_20_SOURCE_DATA: u32 = 2;
    const BIT_24_SOURCE_DATA: u32 = 3;
    const BIT_32_SOURCE_DATA: u32 = 4;
}
*/
#[repr(u32)]
pub enum Mpeg4ObjectId {
    AAC_Main = 1,
    AAC_LC = 2,
    AAC_SSR = 3,
    AAC_LTP = 4,
    AAC_SBR = 5,
    AAC_Scalable = 6,
    TwinVQ = 7,
    CELP = 8,
    HVXC = 9,
}
/*
pub struct AudioTimeStampFlags;
impl AudioTimeStampFlags {
    const SAMPLE_TIME_VALID: u32 = 1;
    const HOST_TIME_VALID: u32 = 2;
    const RATE_SCALAR_VALID: u32 = 4;
    const WORLD_CLOCK_TIME_VALID: u32 = 8;
    const SMPTE_TIME_VALID: u32 = 16;
}
*/
#[derive(Debug, PartialEq, Copy, Clone)]
#[repr(C)]
pub struct CAudioComponentDescription {
    pub componentType: CAudioUnitType,
    pub componentSubType: CAudioUnitSubType,
    pub componentManufacturer: u32,
    pub componentFlags: u32,
    pub componentFlagsMask: u32,
}

impl CAudioComponentDescription {
    pub fn new_apple(ty: CAudioUnitType, sub: CAudioUnitSubType) -> Self {
        Self {
            componentType: ty,
            componentSubType: sub,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        }
    }
    pub fn new_all_manufacturers(ty: CAudioUnitType, sub: CAudioUnitSubType) -> Self {
        Self {
            componentType: ty,
            componentSubType: sub,
            componentManufacturer: 0,
            componentFlags: 0,
            componentFlagsMask: 0,
        }
    }
}

#[derive(Debug, Default)]
#[repr(C)]
pub struct SMPTETime {
    pub mSubframes: i16,
    pub mSubframeDivisor: i16,
    pub mCounter: u32,
    pub mType: u32,
    pub mFlags: u32,
    pub mHours: i16,
    pub mMinutes: i16,
    pub mSeconds: i16,
    pub mFrames: i16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut ::std::os::raw::c_void,
}

pub const MAX_AUDIO_BUFFERS: usize = 8;
#[repr(C)]
pub struct CAudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [_AudioBuffer; MAX_AUDIO_BUFFERS],
}

#[derive(Debug)]
#[repr(C)]
pub struct CAudioTimeStamp {
    pub mSampleTime: f64,
    pub mHostTime: u64,
    pub mRateScalar: f64,
    pub mWordClockTime: u64,
    pub mSMPTETime: SMPTETime,
    pub mFlags: u32,
    pub mReserved: u32,
}

#[derive(Debug, PartialEq, Copy, Clone)]
#[repr(u32)]
pub enum CAudioUnitType {
    IO = 1635086197,
    MusicDevice = 1635085685,
    MusicEffect = 1635085670,
    FormatConverter = 1635083875,
    Effect = 1635083896,
    Mixer = 1635085688,
    Panner = 1635086446,
    Generator = 1635084142,
    OfflineEffect = 1635086188,
}

#[derive(Debug, PartialEq, Copy, Clone)]
#[repr(u32)]
pub enum CAudioUnitSubType {
    Undefined = 0,
    
    PeakLimiter = 1819112562,
    DynamicsProcessor = 1684237680,
    LowPassFilter = 1819304307,
    HighPassFilter = 1752195443,
    BandPassFilter = 1651532147,
    HighShelfFilter = 1752393830,
    LowShelfFilter = 1819502694,
    ParametricEQ = 1886217585,
    Distortion = 1684632436,
    Delay = 1684368505,
    SampleDelay = 1935961209,
    GraphicEQ = 1735550321,
    MultiBandCompressor = 1835232624,
    MatrixReverb = 1836213622,
    Pitch = 1953329268,
    AUFilter = 1718185076,
    NetSend = 1853058660,
    RogerBeep = 1919903602,
    NBandEQ = 1851942257,
    
    //pub enum FormatConverterType
    AUConverter = 1668247158,
    NewTimePitch = 1853191280,
    //TimePitch = 1953329268,
    DeferredRenderer = 1684366962,
    Splitter = 1936747636,
    Merger = 1835364967,
    Varispeed = 1986097769,
    AUiPodTimeOther = 1768977519,
    
    //pub enum MixerType
    MultiChannelMixer = 1835232632,
    StereoMixer = 1936554098,
    Mixer3D = 862219640,
    MatrixMixer = 1836608888,
    
    //pub enum GeneratorType {
    ScheduledSoundPlayer = 1936945260,
    AudioFilePlayer = 1634103404,
    
    //pub enum MusicDeviceType {
    DLSSynth = 1684828960,
    Sampler = 1935764848,
    
    //pub enum IOType {
    GenericOutput = 1734700658,
    HalOutput = 1634230636,
    DefaultOutput = 1684366880,
    SystemOutput = 1937339168,
    VoiceProcessingIO = 1987078511,
    RemoteIO = 1919512419,
}

#[derive(Debug)]
#[repr(i32)]
pub enum OSError {
    
    Unimplemented = -4,
    FileNotFound = -43,
    FilePermission = -54,
    TooManyFilesOpen = -42,
    
    Unspecified = -1500,
    SystemSoundClientMessageTimeout = -1501,
    
    BadFilePath = 561017960,
    Param = -50,
    MemFull = -108,
    
    FormatUnspecified = 2003329396,
    UnknownProperty = 2003332927,
    BadPropertySize = 561211770,
    IllegalOperation = 1852797029,
    UnsupportedFormat = 560226676,
    State = 561214580,
    NotEnoughBufferSpace = 560100710,
    
    UnsupportedDataFormat = 1718449215,
    
    InvalidProperty = -10879,
    InvalidParameter = -10878,
    InvalidElement = -10877,
    NoConnection = -10876,
    FailedInitialization = -10875,
    TooManyFramesToProcess = -10874,
    InvalidFile = -10871,
    FormatNotSupported = -10868,
    Uninitialized = -10867,
    InvalidScope = -10866,
    PropertyNotWritable = -10865,
    CannotDoInCurrentContext = -10863,
    InvalidPropertyValue = -10851,
    PropertyNotInUse = -10850,
    Initialized = -10849,
    InvalidOfflineRender = -10848,
    Unauthorized = -10847,
    
    NoMatchingDefaultAudioUnitFound,
    
    Unknown,
}

pub const kAudioComponentInstantiation_LoadInProcess: u32 = 2;
pub const kAudioComponentInstantiation_LoadOutOfProcess: u32 = 1;

impl OSError {
    pub fn from(result: i32) -> Result<(), Self> {
        Err(match result {
            0 => return Ok(()),
            x if x == Self::Unimplemented as i32 => Self::Unimplemented,
            x if x == Self::FileNotFound as i32 => Self::FileNotFound,
            x if x == Self::FilePermission as i32 => Self::FilePermission,
            x if x == Self::TooManyFilesOpen as i32 => Self::TooManyFilesOpen,
            
            x if x == Self::Unspecified as i32 => Self::Unspecified,
            x if x == Self::SystemSoundClientMessageTimeout as i32 => Self::SystemSoundClientMessageTimeout,
            
            x if x == Self::BadFilePath as i32 => Self::BadFilePath,
            x if x == Self::Param as i32 => Self::Param,
            x if x == Self::MemFull as i32 => Self::MemFull,
            
            x if x == Self::FormatUnspecified as i32 => Self::FormatUnspecified,
            x if x == Self::UnknownProperty as i32 => Self::UnknownProperty,
            x if x == Self::BadPropertySize as i32 => Self::BadPropertySize,
            x if x == Self::IllegalOperation as i32 => Self::IllegalOperation,
            x if x == Self::UnsupportedFormat as i32 => Self::UnsupportedFormat,
            x if x == Self::State as i32 => Self::State,
            x if x == Self::NotEnoughBufferSpace as i32 => Self::NotEnoughBufferSpace,
            
            x if x == Self::UnsupportedDataFormat as i32 => Self::UnsupportedDataFormat,
            
            x if x == Self::InvalidProperty as i32 => Self::InvalidProperty,
            x if x == Self::InvalidParameter as i32 => Self::InvalidParameter,
            x if x == Self::InvalidElement as i32 => Self::InvalidElement,
            x if x == Self::NoConnection as i32 => Self::NoConnection,
            x if x == Self::FailedInitialization as i32 => Self::FailedInitialization,
            x if x == Self::TooManyFramesToProcess as i32 => Self::TooManyFramesToProcess,
            x if x == Self::InvalidFile as i32 => Self::InvalidFile,
            x if x == Self::FormatNotSupported as i32 => Self::FormatNotSupported,
            x if x == Self::Uninitialized as i32 => Self::Uninitialized,
            x if x == Self::InvalidScope as i32 => Self::InvalidScope,
            x if x == Self::PropertyNotWritable as i32 => Self::PropertyNotWritable,
            x if x == Self::CannotDoInCurrentContext as i32 => Self::CannotDoInCurrentContext,
            x if x == Self::InvalidPropertyValue as i32 => Self::InvalidPropertyValue,
            x if x == Self::PropertyNotInUse as i32 => Self::PropertyNotInUse,
            x if x == Self::Initialized as i32 => Self::Initialized,
            x if x == Self::InvalidOfflineRender as i32 => Self::InvalidOfflineRender,
            x if x == Self::Unauthorized as i32 => Self::Unauthorized,
            _ => Self::Unknown
        })
    }
    
    pub fn from_nserror(ns_error: ObjcId) -> Result<(), Self> {
        if ns_error != nil {
            let code: i32 = unsafe {msg_send![ns_error, code]};
            Self::from(code)
        }
        else {
            Ok(())
        }
    }
}

pub type ItemCount = u64;
pub type MIDIObjectRef = u32;
pub type MIDIClientRef = MIDIObjectRef;
pub type MIDIPortRef = MIDIObjectRef;
pub type MIDIEndpointRef = MIDIObjectRef;
pub type MIDIProtocolID = i32;
pub type MIDITimeStamp = u64;
pub const kMIDIProtocol_1_0: i32 = 1;
pub const kMIDIProtocol_2_0: i32 = 2;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct MIDINotification {
    pub messageID: i32,
    pub messageSize: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MIDIEventList {
    pub protocol: MIDIProtocolID,
    pub numPackets: u32,
    pub packet: [MIDIEventPacket; 1usize],
}

#[repr(C, packed(4))]
#[derive(Copy, Clone)]
pub struct MIDIEventPacket {
    pub timeStamp: MIDITimeStamp,
    pub wordCount: u32,
    pub words: [u32; 64usize],
}

#[link(name = "CoreMidi", kind = "framework")]
extern "C" {
    pub static kMIDIPropertyManufacturer: CFStringRef;
    pub static kMIDIPropertyDisplayName: CFStringRef;
    pub static kMIDIPropertyUniqueID: CFStringRef;
    
    pub fn MIDIGetNumberOfSources() -> ItemCount;
    pub fn MIDIGetSource(sourceIndex0: ItemCount) -> MIDIEndpointRef;
    
    pub fn MIDIGetNumberOfDestinations() -> ItemCount;
    pub fn MIDIGetDestination(sourceIndex0: ItemCount) -> MIDIEndpointRef;
    
    pub fn MIDISendEventList(
        port: MIDIPortRef,
        dest: MIDIEndpointRef,
        evtlist: *const MIDIEventList,
    ) -> OSStatus;
    
    pub fn MIDIClientCreateWithBlock(
        name: CFStringRef,
        outClient: *mut MIDIClientRef,
        notifyBlock: ObjcId,
    ) -> OSStatus;
    
    pub fn MIDIInputPortCreateWithProtocol(
        client: MIDIClientRef,
        portName: CFStringRef,
        protocol: MIDIProtocolID,
        outPort: *mut MIDIPortRef,
        receiveBlock: ObjcId,
    ) -> OSStatus;
    
    pub fn MIDIOutputPortCreate(
        client: MIDIClientRef,
        portName: CFStringRef,
        outPort: *mut MIDIPortRef,
    ) -> OSStatus;
    
    pub fn MIDIObjectGetStringProperty(
        obj: MIDIObjectRef,
        propertyID: CFStringRef,
        str_: *mut CFStringRef,
    ) -> OSStatus;
    
    pub fn MIDIObjectGetIntegerProperty(
        obj: MIDIObjectRef,
        propertyID: CFStringRef,
        outValue: *mut i32,
    ) -> OSStatus;
    
    pub fn MIDIPortConnectSource(
        port: MIDIPortRef,
        source: MIDIEndpointRef,
        connRefCon: *mut ::std::os::raw::c_void,
    ) -> OSStatus;
}
