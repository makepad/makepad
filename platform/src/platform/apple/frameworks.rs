// stripped mac core foundation + metal layer only whats needed

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

pub use {
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
    std::{
        ffi::c_void,
        os::raw::c_ulong,
        ptr::NonNull,
    },
};

//use bitflags::bitflags;

pub type ObjcId = *mut Object;
pub const nil: ObjcId = 0 as ObjcId;

pub struct RcObjcId(NonNull<Object>);

impl RcObjcId {
    pub fn from_owned(id: NonNull<Object>) -> Self {
        Self (id)
    }
    
    pub fn from_unowned(id: NonNull<Object>) -> Self {
        unsafe {
            let _: () = msg_send![id.as_ptr(), retain];
        }
        Self::from_owned(id)
    }
    
    pub fn as_id(&self) -> ObjcId {
        self.0.as_ptr()
    }
    
    pub fn forget(self) -> NonNull<Object> {
        unsafe {
            let _: () = msg_send![self.0.as_ptr(), retain];
        }
        self.0
    }
}

impl Clone for RcObjcId {
    fn clone(&self) -> Self {
        Self::from_unowned(self.0)
    }
}

impl Drop for RcObjcId {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.0.as_ptr(), release];
        }
    }
}

#[link(name = "system")]
extern {
    pub static _NSConcreteStackBlock: [*const c_void; 32];
    pub static _NSConcreteBogusBlock: [*const c_void; 32];
}

#[link(name = "Foundation", kind = "framework")]
extern {
    pub static NSRunLoopCommonModes: ObjcId;
    pub static NSDefaultRunLoopMode: ObjcId;
}

#[link(name = "AppKit", kind = "framework")]
extern {
    pub static NSPasteboardURLReadingFileURLsOnlyKey: ObjcId;
    
    pub static NSStringPboardType: ObjcId;
    pub static NSPasteboardTypeFileURL: ObjcId;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayPixelsHigh(display: u32) -> u64;
    pub fn CGColorCreateGenericRGB(red: f64, green: f64, blue: f64, alpha: f64) -> ObjcId;
}

#[link(name = "Metal", kind = "framework")]
extern "C" {
    fn MTLCreateSystemDefaultDevice() -> ObjcId;
    #[cfg(not(target_os = "ios"))]
    fn MTLCopyAllDevices() -> ObjcId; //TODO: Array
}


// HELPERS



pub fn get_default_metal_device() -> Option<ObjcId> {
    unsafe {
        let dev = MTLCreateSystemDefaultDevice();
        if dev == nil {None} else {Some(dev)}
    }
}


pub fn get_all_metal_devices() -> Vec<ObjcId> {
    #[cfg(target_os = "ios")]
    {
        MTLCreateSystemDefaultDevice().into_iter().collect()
    }
    #[cfg(not(target_os = "ios"))]
    unsafe {
        let array = MTLCopyAllDevices();
        let count: u64 = msg_send![array, count];
        let ret = (0..count)
            .map( | i | msg_send![array, objectAtIndex: i])
        // The elements of this array are references---we convert them to owned references
        // (which just means that we increment the reference count here, and it is
        // decremented in the `Drop` impl for `Device`)
            .map( | device: *mut Object | msg_send![device, retain])
            .collect();
        let () = msg_send![array, release];
        ret
    }
}


pub fn nsstring_to_string(string: ObjcId) -> String {
    unsafe {
        let utf8_string: *const std::os::raw::c_uchar = msg_send![string, UTF8String];
        let utf8_len: usize = msg_send![string, lengthOfBytesUsingEncoding: UTF8_ENCODING];
        let slice = std::slice::from_raw_parts(
            utf8_string,
            utf8_len,
        );
        std::str::from_utf8_unchecked(slice).to_owned()
    }
}

pub fn str_to_ns_string(str: &str) -> ObjcId {
    unsafe {
        let ns_string: ObjcId = msg_send![class!(NSString), alloc];
        let ns_string: ObjcId = msg_send![
            ns_string,
            initWithBytes: str.as_ptr()
            length: str.len()
            encoding: UTF8_ENCODING as ObjcId
        ];
        let _: () = msg_send![ns_string, autorelease];
        ns_string
    }
}

pub fn load_native_cursor(cursor_name: &str) -> ObjcId {
    let sel = Sel::register(cursor_name);
    let id: ObjcId = unsafe {msg_send![class!(NSCursor), performSelector: sel]};
    id
}

pub fn load_undocumented_cursor(cursor_name: &str) -> ObjcId {
    unsafe {
        let class = class!(NSCursor);
        let sel = Sel::register(cursor_name);
        let sel: ObjcId = msg_send![class, respondsToSelector: sel];
        let id: ObjcId = msg_send![class, performSelector: sel];
        id
    }
}

pub fn load_webkit_cursor(cursor_name_str: &str) -> ObjcId {
    unsafe {
        static CURSOR_ROOT: &'static str = "/System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/Resources/cursors";
        let cursor_root = str_to_ns_string(CURSOR_ROOT);
        let cursor_name = str_to_ns_string(cursor_name_str);
        let cursor_pdf = str_to_ns_string("cursor.pdf");
        let cursor_plist = str_to_ns_string("info.plist");
        let key_x = str_to_ns_string("hotx");
        let key_y = str_to_ns_string("hoty");
        
        let cursor_path: ObjcId = msg_send![cursor_root, stringByAppendingPathComponent: cursor_name];
        let pdf_path: ObjcId = msg_send![cursor_path, stringByAppendingPathComponent: cursor_pdf];
        let info_path: ObjcId = msg_send![cursor_path, stringByAppendingPathComponent: cursor_plist];
        
        let ns_image: ObjcId = msg_send![class!(NSImage), alloc];
        let () = msg_send![ns_image, initByReferencingFile: pdf_path];
        let info: ObjcId = msg_send![class!(NSDictionary), dictionaryWithContentsOfFile: info_path];
        //let image = NSImage::alloc(nil).initByReferencingFile_(pdf_path);
        // let info = NSDictionary::dictionaryWithContentsOfFile_(nil, info_path);
        
        let x: ObjcId = msg_send![info, valueForKey: key_x]; //info.valueForKey_(key_x);
        let y: ObjcId = msg_send![info, valueForKey: key_y]; //info.valueForKey_(key_y);
        let point = NSPoint {
            x: msg_send![x, doubleValue],
            y: msg_send![y, doubleValue],
        };
        let cursor: ObjcId = msg_send![class!(NSCursor), alloc];
        msg_send![cursor, initWithImage: ns_image hotSpot: point]
    }
}




// COCOA



#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct NSPoint {
    pub x: f64,
    pub y: f64,
}

unsafe impl Encode for NSPoint {
    fn encode() -> Encoding {
        let encoding = format!("{{CGPoint={}{}}}", f64::encode().as_str(), f64::encode().as_str());
        unsafe {Encoding::from_str(&encoding)}
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct NSSize {
    pub width: f64,
    pub height: f64,
}

unsafe impl Encode for NSSize {
    fn encode() -> Encoding {
        let encoding = format!("{{CGSize={}{}}}", f64::encode().as_str(), f64::encode().as_str());
        unsafe {Encoding::from_str(&encoding)}
    }
}


#[repr(C)]
#[derive(Copy, Clone)]
pub struct NSRect {
    pub origin: NSPoint,
    pub size: NSSize,
}

unsafe impl Encode for NSRect {
    fn encode() -> Encoding {
        let encoding = format!("{{CGRect={}{}}}", NSPoint::encode().as_str(), NSSize::encode().as_str());
        unsafe {Encoding::from_str(&encoding)}
    }
}

#[repr(u64)] // NSUInteger
pub enum NSEventModifierFlags {
    NSAlphaShiftKeyMask = 1 << 16,
    NSShiftKeyMask = 1 << 17,
    NSControlKeyMask = 1 << 18,
    NSAlternateKeyMask = 1 << 19,
    NSCommandKeyMask = 1 << 20,
    NSNumericPadKeyMask = 1 << 21,
    NSHelpKeyMask = 1 << 22,
    NSFunctionKeyMask = 1 << 23,
    NSDeviceIndependentModifierFlagsMask = 0xffff0000
}

const UTF8_ENCODING: usize = 4;

#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NSWindowTitleVisibility {
    NSWindowTitleVisible = 0,
    NSWindowTitleHidden = 1
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u64)] // NSUInteger
pub enum NSEventType {
    NSLeftMouseDown = 1,
    NSLeftMouseUp = 2,
    NSRightMouseDown = 3,
    NSRightMouseUp = 4,
    NSMouseMoved = 5,
    NSLeftMouseDragged = 6,
    NSRightMouseDragged = 7,
    NSMouseEntered = 8,
    NSMouseExited = 9,
    NSKeyDown = 10,
    NSKeyUp = 11,
    NSFlagsChanged = 12,
    NSAppKitDefined = 13,
    NSSystemDefined = 14,
    NSApplicationDefined = 15,
    NSPeriodic = 16,
    NSCursorUpdate = 17,
    NSScrollWheel = 22,
    NSTabletPoint = 23,
    NSTabletProximity = 24,
    NSOtherMouseDown = 25,
    NSOtherMouseUp = 26,
    NSOtherMouseDragged = 27,
    NSEventTypeGesture = 29,
    NSEventTypeMagnify = 30,
    NSEventTypeSwipe = 31,
    NSEventTypeRotate = 18,
    NSEventTypeBeginGesture = 19,
    NSEventTypeEndGesture = 20,
    NSEventTypePressure = 34,
}

#[repr(u64)] // NSUInteger
pub enum NSEventMask {
    NSLeftMouseDownMask = 1 << NSEventType::NSLeftMouseDown as u64,
    NSLeftMouseUpMask = 1 << NSEventType::NSLeftMouseUp as u64,
    NSRightMouseDownMask = 1 << NSEventType::NSRightMouseDown as u64,
    NSRightMouseUpMask = 1 << NSEventType::NSRightMouseUp as u64,
    NSMouseMovedMask = 1 << NSEventType::NSMouseMoved as u64,
    NSLeftMouseDraggedMask = 1 << NSEventType::NSLeftMouseDragged as u64,
    NSRightMouseDraggedMask = 1 << NSEventType::NSRightMouseDragged as u64,
    NSMouseEnteredMask = 1 << NSEventType::NSMouseEntered as u64,
    NSMouseExitedMask = 1 << NSEventType::NSMouseExited as u64,
    NSKeyDownMask = 1 << NSEventType::NSKeyDown as u64,
    NSKeyUpMask = 1 << NSEventType::NSKeyUp as u64,
    NSFlagsChangedMask = 1 << NSEventType::NSFlagsChanged as u64,
    NSAppKitDefinedMask = 1 << NSEventType::NSAppKitDefined as u64,
    NSSystemDefinedMask = 1 << NSEventType::NSSystemDefined as u64,
    NSApplicationDefinedMask = 1 << NSEventType::NSApplicationDefined as u64,
    NSPeriodicMask = 1 << NSEventType::NSPeriodic as u64,
    NSCursorUpdateMask = 1 << NSEventType::NSCursorUpdate as u64,
    NSScrollWheelMask = 1 << NSEventType::NSScrollWheel as u64,
    NSTabletPointMask = 1 << NSEventType::NSTabletPoint as u64,
    NSTabletProximityMask = 1 << NSEventType::NSTabletProximity as u64,
    NSOtherMouseDownMask = 1 << NSEventType::NSOtherMouseDown as u64,
    NSOtherMouseUpMask = 1 << NSEventType::NSOtherMouseUp as u64,
    NSOtherMouseDraggedMask = 1 << NSEventType::NSOtherMouseDragged as u64,
    NSEventMaskGesture = 1 << NSEventType::NSEventTypeGesture as u64,
    NSEventMaskSwipe = 1 << NSEventType::NSEventTypeSwipe as u64,
    NSEventMaskRotate = 1 << NSEventType::NSEventTypeRotate as u64,
    NSEventMaskBeginGesture = 1 << NSEventType::NSEventTypeBeginGesture as u64,
    NSEventMaskEndGesture = 1 << NSEventType::NSEventTypeEndGesture as u64,
    NSEventMaskPressure = 1 << NSEventType::NSEventTypePressure as u64,
    NSAnyEventMask = 0xffffffffffffffff
}

#[repr(u64)] // NSUInteger
pub enum NSWindowStyleMask {
    NSBorderlessWindowMask = 0,
    NSTitledWindowMask = 1 << 0,
    NSClosableWindowMask = 1 << 1,
    NSMiniaturizableWindowMask = 1 << 2,
    NSResizableWindowMask = 1 << 3,
    
    NSTexturedBackgroundWindowMask = 1 << 8,
    
    NSUnifiedTitleAndToolbarWindowMask = 1 << 12,
    
    NSFullScreenWindowMask = 1 << 14,
    
    NSFullSizeContentViewWindowMask = 1 << 15,
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NSApplicationActivationOptions {
    NSApplicationActivateAllWindows = 1 << 0,
    NSApplicationActivateIgnoringOtherApps = 1 << 1
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NSApplicationActivationPolicy {
    NSApplicationActivationPolicyRegular = 0,
    NSApplicationActivationPolicyAccessory = 1,
    NSApplicationActivationPolicyProhibited = 2,
    NSApplicationActivationPolicyERROR = -1
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NSBackingStoreType {
    NSBackingStoreRetained = 0,
    NSBackingStoreNonretained = 1,
    NSBackingStoreBuffered = 2
}

#[repr(C)]
pub struct NSRange {
    pub location: u64,
    pub length: u64,
}
/*
impl NSRange {
    #[inline]
    pub fn new(location: u64, length: u64) -> NSRange {
        NSRange {location, length}
    }
}*/

unsafe impl Encode for NSRange {
    fn encode() -> Encoding {
        let encoding = format!(
            // TODO: Verify that this is correct
            "{{NSRange={}{}}}",
            u64::encode().as_str(),
            u64::encode().as_str(),
        );
        unsafe {Encoding::from_str(&encoding)}
    }
}

pub trait NSMutableAttributedString: Sized {
    unsafe fn alloc(_: Self) -> ObjcId {
        msg_send![class!(NSMutableAttributedString), alloc]
    }
    
    unsafe fn init(self) -> ObjcId;
    // *mut NSMutableAttributedString
    unsafe fn init_with_string(self, string: ObjcId) -> ObjcId;
    unsafe fn init_with_attributed_string(self, string: ObjcId) -> ObjcId;
    
    unsafe fn string(self) -> ObjcId;
    // *mut NSString
    unsafe fn mutable_string(self) -> ObjcId;
    // *mut NSMutableString
    unsafe fn length(self) -> u64;
}

impl NSMutableAttributedString for ObjcId {
    unsafe fn init(self) -> ObjcId {
        msg_send![self, init]
    }
    
    unsafe fn init_with_string(self, string: ObjcId) -> ObjcId {
        msg_send![self, initWithString: string]
    }
    
    unsafe fn init_with_attributed_string(self, string: ObjcId) -> ObjcId {
        msg_send![self, initWithAttributedString: string]
    }
    
    unsafe fn string(self) -> ObjcId {
        msg_send![self, string]
    }
    
    unsafe fn mutable_string(self) -> ObjcId {
        msg_send![self, mutableString]
    }
    
    unsafe fn length(self) -> u64 {
        msg_send![self, length]
    }
}



// Metal API




#[repr(u64)]
#[derive(Clone, Debug)]
pub enum MTLLoadAction {
    DontCare = 0,
    Load = 1,
    Clear = 2,
}

#[repr(u64)]
#[derive(Clone, Debug)]
pub enum MTLStoreAction {
    DontCare = 0,
    Store = 1,
    MultisampleResolve = 2,
    StoreAndMultisampleResolve = 3,
    Unknown = 4,
    CustomSampleDepthStore = 5,
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct MTLClearColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum MTLPixelFormat {
    BGRA8Unorm = 80,
    Depth32Float = 252,
    Stencil8 = 253,
    Depth24Unorm_Stencil8 = 255,
    Depth32Float_Stencil8 = 260,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MTLBlendFactor {
    Zero = 0,
    One = 1,
    SourceColor = 2,
    OneMinusSourceColor = 3,
    SourceAlpha = 4,
    OneMinusSourceAlpha = 5,
    DestinationColor = 6,
    OneMinusDestinationColor = 7,
    DestinationAlpha = 8,
    OneMinusDestinationAlpha = 9,
    SourceAlphaSaturated = 10,
    BlendColor = 11,
    OneMinusBlendColor = 12,
    BlendAlpha = 13,
    OneMinusBlendAlpha = 14,
    Source1Color = 15,
    OneMinusSource1Color = 16,
    Source1Alpha = 17,
    OneMinusSource1Alpha = 18,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MTLBlendOperation {
    Add = 0,
    Subtract = 1,
    ReverseSubtract = 2,
    Min = 3,
    Max = 4,
}

#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum MTLPrimitiveType {
    Point = 0,
    Line = 1,
    LineStrip = 2,
    Triangle = 3,
    TriangleStrip = 4,
}

#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum MTLIndexType {
    UInt16 = 0,
    UInt32 = 1,
}

#[repr(u64)]
pub enum MTLCompareFunction {
    Never = 0,
    Less = 1,
    Equal = 2,
    LessEqual = 3,
    Greater = 4,
    NotEqual = 5,
    GreaterEqual = 6,
    Always = 7,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum MTLTextureType {
    D1 = 0,
    D1Array = 1,
    D2 = 2,
    D2Array = 3,
    D2Multisample = 4,
    Cube = 5,
    CubeArray = 6,
    D3 = 7,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
pub enum MTLTextureUsage {
    Unknown = 0x0000,
    ShaderRead = 0x0001,
    ShaderWrite = 0x0002,
    RenderTarget = 0x0004,
    PixelFormatView = 0x0010,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MTLStorageMode {
    Shared = 0,
    Managed = 1,
    Private = 2,
    Memoryless = 3,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MTLOrigin {
    pub x: u64,
    pub y: u64,
    pub z: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MTLSize {
    pub width: u64,
    pub height: u64,
    pub depth: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct MTLRegion {
    pub origin: MTLOrigin,
    pub size: MTLSize,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MTLCPUCacheMode {
    DefaultCache = 0,
    WriteCombined = 1,
}

#[repr(u64)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MTLHazardTrackingMode {
    Default = 0,
    Untracked = 1,
    Tracked = 2,
}

pub const MTLResourceCPUCacheModeShift: u64 = 0;
pub const MTLResourceCPUCacheModeMask: u64 = 0xf << MTLResourceCPUCacheModeShift;

pub const MTLResourceStorageModeShift: u64 = 4;
pub const MTLResourceStorageModeMask: u64 = 0xf << MTLResourceStorageModeShift;

pub const MTLResourceHazardTrackingModeShift: u64 = 8;
pub const MTLResourceHazardTrackingModeMask: u64 = 0x3 << MTLResourceHazardTrackingModeShift;

#[allow(non_upper_case_globals)]
pub struct MTLResourceOptions;
impl MTLResourceOptions {
    pub const CPUCacheModeDefaultCache: u64 = (MTLCPUCacheMode::DefaultCache as u64) << MTLResourceCPUCacheModeShift;
    pub const CPUCacheModeWriteCombined: u64 = (MTLCPUCacheMode::WriteCombined as u64) << MTLResourceCPUCacheModeShift;
    
    pub const HazardTrackingModeUntracked: u64 = (MTLHazardTrackingMode::Untracked as u64) << MTLResourceHazardTrackingModeShift;
    pub const HazardTrackingModeTracked: u64 = (MTLHazardTrackingMode::Tracked as u64) << MTLResourceHazardTrackingModeShift;
    
    pub const StorageModeShared: u64 = (MTLStorageMode::Shared as u64) << MTLResourceStorageModeShift;
    pub const StorageModeManaged: u64 = (MTLStorageMode::Managed as u64) << MTLResourceStorageModeShift;
    pub const StorageModePrivate: u64 = (MTLStorageMode::Private as u64) << MTLResourceStorageModeShift;
    pub const StorageModeMemoryless: u64 = (MTLStorageMode::Memoryless as u64) << MTLResourceStorageModeShift;
}

#[repr(u64)]
pub enum NSDragOperation {
    None = 0,
    Copy = 1,
    Link = 2,
    Move = 16,
}

unsafe impl Encode for NSDragOperation {
    fn encode() -> Encoding {
        let encoding = format!("Q");
        unsafe {Encoding::from_str(&encoding)}
    }
}



// CORE AUDIO


pub const kAudioUnitManufacturer_Apple: u32 = 1634758764;

#[repr(C)] pub struct OpaqueAudioComponent([u8; 0]);
pub type AudioComponent = *mut OpaqueAudioComponent;

#[repr(C)] pub struct ComponentInstanceRecord([u8; 0]);
pub type AudioComponentInstance = *mut ComponentInstanceRecord;
pub type AudioUnit = AudioComponentInstance;

pub type OSStatus = i32;

#[repr(C)] 
pub struct AudioStreamBasicDescription {
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
pub enum AudioFormatId{
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

struct F60958AC3Flags;
impl F60958AC3Flags{
    const IS_FLOAT:u32 = 1;
    const IS_BIG_ENDIAN:u32 = 2;
    const IS_SIGNED_INTEGER:u32 = 4;
    const IS_PACKED:u32 = 8;
    const IS_ALIGNED_HIGH:u32 = 16;
    const IS_NON_INTERLEAVED:u32 = 32;
    const IS_NON_MIXABLE:u32 = 64;
}


pub struct LinearPcmFlags;
impl LinearPcmFlags{
    const IS_FLOAT:u32 = 1;
    const IS_BIG_ENDIAN:u32 = 2;
    const IS_SIGNED_INTEGER:u32 = 4;
    const IS_PACKED:u32 = 8;
    const IS_ALIGNED_HIGH:u32 = 16;
    const IS_NON_INTERLEAVED:u32 = 32;
    const IS_NON_MIXABLE:u32 = 64;
    const FLAGS_SAMPLE_FRACTION_SHIFT:u32 = 7;
    const FLAGS_SAMPLE_FRACTION_MASK:u32 = 8064;
}

pub struct AppleLosslessFlags;
impl AppleLosslessFlags{
    const BIT_16_SOURCE_DATA:u32 = 1;
    const BIT_20_SOURCE_DATA:u32 = 2;
    const BIT_24_SOURCE_DATA:u32 = 3;
    const BIT_32_SOURCE_DATA:u32 = 4;    
}

#[repr(u32)]
pub enum Mpeg4ObjectId{
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

pub struct AudioTimeStampFlags;
impl AudioTimeStampFlags {
    const SAMPLE_TIME_VALID:u32 = 1;
    const HOST_TIME_VALID:u32 = 2;
    const RATE_SCALAR_VALID:u32 = 4;
    const WORLD_CLOCK_TIME_VALID:u32 = 8;
    const SMPTE_TIME_VALID:u32 = 16;
}

#[repr(C)]
pub struct AudioComponentDescription {
    pub componentType: AudioUnitType,
    pub componentSubType: AudioUnitSubType,
    pub componentManufacturer: u32,
    pub componentFlags: u32,
    pub componentFlagsMask: u32,
}

#[repr(u32)]
pub enum AudioUnitType {
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

#[repr(u32)]
pub enum AudioUnitSubType {
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
pub enum AudioError {
    
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

impl AudioError {
    pub fn result(val: i32) -> Result<(),Self> {
        Err(match val {
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
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    
    pub fn AudioComponentFindNext(
        inComponent: AudioComponent,
        inDesc: *const AudioComponentDescription,
    ) -> AudioComponent;
    
    pub fn AudioComponentInstanceNew(
        inComponent: AudioComponent,
        outInstance: *mut AudioComponentInstance,
    ) -> OSStatus;
    
    pub fn AudioUnitInitialize(inUnit: AudioUnit) -> OSStatus;
}
