// stripped mac core foundation + metal layer only whats needed

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(non_snake_case)]

pub use {
    makepad_objc_sys::{
        runtime::{Class, Object, Protocol, Sel, BOOL, YES, NO,},
        declare::{ClassDecl, ProtocolDecl},
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
    pub static NSProcessInfo: ObjcId;
    pub fn NSStringFromClass(class:ObjcId)->ObjcId;
    
    pub fn __CFStringMakeConstantString(cStr: *const ::std::os::raw::c_char) -> CFStringRef;
    
    pub fn CFStringGetLength(theString: CFStringRef) -> u64;
    pub fn CFStringGetBytes(
        theString: CFStringRef,
        range: CFRange,
        encoding: u32,
        lossByte: u8,
        isExternalRepresentation: bool,
        buffer: *mut u8,
        maxBufLen: u64,
        usedBufLen: *mut u64,
    ) -> u64;

}

#[link(name = "ImageIO", kind = "framework")]
extern{
    pub static kUTTypePNG:ObjcId;
    pub fn CGImageDestinationCreateWithURL(url:ObjcId, ty:ObjcId, count:u64, options:ObjcId)->ObjcId;    
    pub fn CGImageDestinationAddImage(dest:ObjcId, img:ObjcId, props:ObjcId);    
    pub fn CGImageDestinationFinalize(dest:ObjcId)->bool;    
}

#[link(name = "AppKit", kind = "framework")]
extern {
    pub static NSPasteboardURLReadingFileURLsOnlyKey: ObjcId;
    pub static NSTrackingArea: ObjcId;
    pub static NSStringPboardType: ObjcId;
    pub static NSPasteboardTypeFileURL: ObjcId;
}

#[link(name = "Vision", kind = "framework")]
extern {
    pub static VNImageRequestHandler: ObjcId;
    pub static VNRecognizeTextRequest: ObjcId;
}


pub const kCGEventLeftMouseDown:u32 = 1;
pub const kCGEventLeftMouseUp:u32 = 2;
pub const kCGMouseEventClickState: u32 = 1;
//pub const kCGEventSourceStateHIDSystemState: u32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub fn CGEventSourceCreate(state_id:u32)->ObjcId;
    pub fn CGEventSetIntegerValueField(event:ObjcId, field: u32, value:u64);
    pub fn CGEventCreateMouseEvent(source:ObjcId, mouse_type: u32, pos:NSPoint, button:u32)->ObjcId;
    pub fn CGEventCreateScrollWheelEvent(source:ObjcId, is_line:u32, wheel_count: u32, wheel1: i32, wheel2: i32, wheel3: i32)->ObjcId;
    pub fn CGEventPostToPid(pid:u32, event:ObjcId);
    pub fn CGEventPost(tap:u32, event:ObjcId);
    
    pub fn CGWindowListCreateImage(rect:NSRect, options:u32, window_id:u32, imageoptions:u32 )->ObjcId;
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayPixelsHigh(display: u32) -> u64;
    pub fn CGColorCreateGenericRGB(red: f64, green: f64, blue: f64, alpha: f64) -> ObjcId;
}

#[link(name = "Metal", kind = "framework")]
extern "C" {
    pub fn MTLCreateSystemDefaultDevice() -> ObjcId;
    #[cfg(not(target_os = "ios"))]
    pub fn MTLCopyAllDevices() -> ObjcId; //TODO: Array
}

#[link(name = "AVFoundation", kind = "framework")]
extern {
    pub static AVAudioUnitComponentManager: ObjcId;
    pub static AVAudioUnit: ObjcId;
}


// Foundation



#[repr(C)]
#[derive(Copy, Clone)]
pub struct CFRange {
    pub location: u64,
    pub length: u64,
}

pub const kCFStringEncodingUTF8: u32 = 134217984;
pub const kCGWindowListOptionIncludingWindow: u32 = 1<<3;
pub const kCGWindowImageBoundsIgnoreFraming: u32 = 1<<0;

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
#[derive(Copy, Debug, Clone)]
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
#[derive(Copy, Debug, Clone)]
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

pub const NSTrackignActiveAlways: u64 = 0x80;
pub const NSTrackingInVisibleRect: u64 = 0x200;
pub const NSTrackingMouseEnteredAndExited: u64 = 0x01;
pub const NSTrackingMouseMoved: u64 = 0x02;
pub const NSTrackingCursorUpdate: u64 = 0x04;

pub const UTF8_ENCODING: usize = 4;

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
    RGBA8Unorm = 70,
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

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __CFString {_unused: [u8; 0]}
pub type CFStringRef = *const __CFString;

