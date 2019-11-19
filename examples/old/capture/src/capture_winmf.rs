#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use render::*;

use winapi::shared::minwindef::{DWORD, BYTE, BOOL, UINT, TRUE};
use winapi::ctypes::c_void;
use winapi::RIDL;
use winapi::Interface;
use winapi::DEFINE_GUID;
use winapi::ENUM;
use wio::com::ComPtr;
use winapi::um::unknwnbase::{IUnknown, IUnknownVtbl};
use winapi::um::propidl::{PROPVARIANT, REFPROPVARIANT};
use winapi::um::winnt::{HRESULT, LPCWSTR, LONGLONG, LONG, LPWSTR};
use winapi::shared::winerror::{S_OK};
use winapi::shared::winerror;
use winapi::shared::guiddef::{GUID, REFIID, REFGUID};
use winapi::shared::wtypes::{VT_UI4, VT_UI8, VT_R8, VT_CLSID, VT_LPWSTR, VT_VECTOR, VT_UI1, VT_UNKNOWN};
use std::ptr;
use std::mem;
use com_impl::{Refcount, VTable};
use core::arch::x86_64::*;

#[derive(Default)]
pub struct Capture {
    pub initialized: bool,
    pub texture: Texture,
    pub platform: Option<CapturePlatform>
}

pub struct CapturePlatform {
    pub width: usize,
    pub height: usize,
    pub format: CaptureFormat,
    pub cxthread: CxThread,
    pub texture: Texture,
    pub frame_signal: Signal,
    pub source: ComPtr<IMFMediaSource>,
    pub reader: ComPtr<IMFSourceReader>,
}

#[derive(PartialEq)]
pub enum CaptureFormat {
    YUY2,
    NV12,
    //   RGB24,
    //   RGB32 todo
}

pub enum CaptureEvent {
    NewFrame,
    None
}

impl Capture {
    
    fn mf_create_attributes(num_attrs: DWORD) -> Result<ComPtr<IMFAttributes>, HRESULT> {
        let mut attributes = ptr::null_mut();
        let hr = unsafe {MFCreateAttributes(
            &mut attributes as *mut *mut _,
            num_attrs
        )};
        if winerror::SUCCEEDED(hr) {Ok(unsafe {ComPtr::from_raw(attributes as *mut _)})} else {Err(hr)}
    }
    
    fn mf_create_source_reader(attributes: &ComPtr<IMFAttributes>, source: &ComPtr<IMFMediaSource>) -> Result<ComPtr<IMFSourceReader>, HRESULT> {
        let mut source_reader = ptr::null_mut();
        let hr = unsafe {MFCreateSourceReaderFromMediaSource(
            source.as_raw(),
            attributes.as_raw(),
            &mut source_reader as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {Ok(unsafe {ComPtr::from_raw(source_reader as *mut _)})} else {Err(hr)}
    }
    
    unsafe fn select_format(reader: &ComPtr<IMFSourceReader>, pref_format: &CaptureFormat, pref_width: usize, pref_height: usize, pref_fps: f64) -> Option<ComPtr<IMFMediaType>> {
        let mut counter = 0;
        loop {
            let mut native_type_raw = ptr::null_mut();
            if reader.GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM, counter, &mut native_type_raw) == S_OK {
                let native_type: ComPtr<IMFMediaType> = ComPtr::from_raw(native_type_raw as *mut _);
                let mut guid = mem::uninitialized();
                if native_type.GetGUID(&MF_MT_SUBTYPE, &mut guid) != S_OK {
                    continue;
                }
                
                let mut frame_size: u64 = 0;
                check("native_type.GetUINT64", native_type.GetUINT64(&MF_MT_FRAME_SIZE, &mut frame_size));
                let height = frame_size & 0xffffffff;
                let width = frame_size>>32;
                
                let mut frame_rate: u64 = 0;
                check("native_type.GetDouble", native_type.GetUINT64(&MF_MT_FRAME_RATE, &mut frame_rate));
                let fps = (frame_rate >> 32) as f64 / ((frame_rate & 0xffffffff)as f64);
                if (fps - pref_fps).abs()<0.0001 && pref_width == width as usize && pref_height == height as usize {
                    if guid_equal(&guid, &MFVideoFormat_YUY2) && *pref_format == CaptureFormat::YUY2
                        || guid_equal(&guid, &MFVideoFormat_NV12) && *pref_format == CaptureFormat::NV12
                    //|| guid_equal(&guid, &MFVideoFormat_RGB24) && pref_convert == CapConvert::RGB24
                    //|| guid_equal(&guid, &MFVideoFormat_RGB32) && pref_convert == CapConvert::RGB32
                    {
                        return Some(native_type);
                    }
                }
            }
            else {
                break
            }
            counter += 1;
        }
        return None
    }
    
    pub fn handle_signal(&mut self, _cx: &mut Cx, event: &mut Event) -> CaptureEvent {
        if let Some(platform) = &self.platform {
            if let Event::Signal(se) = event {
                if platform.frame_signal.is_signal(se) { // we haz new texture
                    //let last_time = self.last_time;
                    //let new_time = Cx::profile_time_ns();
                    //println!("{}",1000000000./(new_time-last_time) as f64);
                    //self.last_time = new_time;
                    //let mut front = self.image_front.lock().unwrap();
                    //let cxtex = &mut cx.textures[self.texture.texture_id.unwrap()];
                    //std::mem::swap(&mut (*front), &mut cxtex.image_u32);
                    //cxtex.update_image = true;
                    return CaptureEvent::NewFrame
                }
            }
        }
        return CaptureEvent::None
    }
    
    pub fn init(&mut self, cx: &mut Cx, device_id: usize, pref_format: CaptureFormat, pref_width: usize, pref_height: usize, pref_fps: f64) -> bool {
        
        check("cannot mf_startup", unsafe {
            MFStartup(0)
        });
        
        let attributes = Self::mf_create_attributes(1).expect("cannot mf_create_attributes");
        
        check("attributes.SetGUID", unsafe {
            attributes.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID
            )
        });
        
        unsafe {
            let mut devices_raw: *mut *mut IMFActivate = 0 as _;
            let mut count: u32 = 0;
            check("attributes.SetGUID", MFEnumDeviceSources(attributes.as_raw() as *mut _, &mut devices_raw, &mut count));
            
            let devices = std::slice::from_raw_parts(devices_raw, count as usize);
            
            let mut source_raw = ptr::null_mut();
            if device_id as u32 >= count {
                println!("Capture device with id {} not found", device_id);
                return false
            }
            check("ActivateObject", (*devices[device_id]).ActivateObject(
                &IMFMediaSource::uuidof(),
                &mut source_raw
            ));
            
            let source: ComPtr<IMFMediaSource> = ComPtr::from_raw(source_raw as *mut _);
            
            let attributes = Self::mf_create_attributes(2).expect("cannot mf_create_attributes");
            
            check("attributes.SetUINT32", attributes.SetUINT32(
                &MF_READWRITE_DISABLE_CONVERTERS,
                TRUE as u32
            ));
            
            let source_reader = SourceReaderCallback::new(self);
            check("attributes.SetUnknown", attributes.SetUnknown(
                &MF_SOURCE_READER_ASYNC_CALLBACK,
                source_reader.as_raw() as *mut _
            ));
            
            let reader = Self::mf_create_source_reader(&attributes, &source).expect("cannot mf_create_source_reader");
            // set reader async callback
            
            if let Some(media_type) = Self::select_format(&reader, &pref_format, pref_width, pref_height, pref_fps) {
                //println!("Selected format!");
                check("SetCurrentMediaType", reader.SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                    ptr::null(),
                    media_type.as_raw()
                ));
                check("ReadSample", reader.ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM,
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut()
                ));
                
                self.texture.set_desc(cx, Some(TextureDesc {
                    format: TextureFormat::MappedBGRA,
                    width: Some(pref_width),
                    height: Some(pref_height),
                    multisample: None
                }));
                
                self.platform = Some(CapturePlatform {
                    format: pref_format,
                    width: pref_width as usize,
                    height: pref_height as usize,
                    texture: self.texture.clone(),
                    source: source,
                    reader: reader,
                    cxthread: cx.new_cxthread(),
                    frame_signal: cx.new_signal(),
                });
                self.initialized = true;
                return true
            }
            else {
                println!("FORMAT NOT FOUND");
            }
        }
        return false;
    }
}

#[repr(C)]
#[derive(com_impl::ComImpl)]
pub struct SourceReaderCallback {
    vtbl: VTable<IMFSourceReaderCallbackVtbl>,
    refcount: Refcount,
    capture: *mut Capture,
}

impl SourceReaderCallback {
    unsafe fn new(capture: *mut Capture) -> ComPtr<IMFSourceReaderCallback> {
        let ptr = SourceReaderCallback::create_raw(capture);
        let ptr = ptr as *mut IMFSourceReaderCallback;
        ComPtr::from_raw(ptr)
    }
}

#[com_impl::com_impl]
unsafe impl IMFSourceReaderCallback for SourceReaderCallback {
    unsafe fn on_read_sample(&self, hrState: HRESULT, _dwStreamIndex: DWORD, _dwStreamFlags: DWORD, _llTimeStamp: LONGLONG, pSample: *const IMFSample,) -> HRESULT {
        if hrState != S_OK {
            println!("read sample returned failure!");
            return S_OK;
        }
        
        let capture_platform = (*self.capture).platform.as_mut().unwrap();
        let width = capture_platform.width;
        let height = capture_platform.height;
        
        if pSample != ptr::null() {
            let mut media_buffer_raw = ptr::null_mut();
            if (*pSample).GetBufferByIndex(0, &mut media_buffer_raw as *mut *mut _) == S_OK {
                let media_buffer: ComPtr<IMFMediaBuffer> = ComPtr::from_raw(media_buffer_raw as *mut _);
                // lets query it for the 2D Buffer interface
                let mut media_buffer2d_raw = ptr::null_mut();
                if media_buffer.QueryInterface(&IMF2DBuffer::uuidof(), &mut media_buffer2d_raw as *mut *mut _) == S_OK {
                    let media_buffer2d: ComPtr<IMF2DBuffer> = ComPtr::from_raw(media_buffer2d_raw as *mut _);
                    let mut stride: LONG = 0;
                    let mut scanline0: *mut BYTE = ptr::null_mut();
                    if media_buffer2d.Lock2D(&mut scanline0, &mut stride) == S_OK {
                        
                        if let Some(out_u32) = capture_platform.cxthread.lock_mapped_texture_u32(&capture_platform.texture, 0) {
                            
                            match capture_platform.format {
                                CaptureFormat::YUY2 => {
                                    // lets convert YUY2 to RGB
                                    let in_buf = std::slice::from_raw_parts(scanline0 as *mut u16, width * height);
                                    for y in 0..height {
                                        for x in (0..width).step_by(2 * 4) {
                                            let off = y * width + x;
                                            
                                            let ay0 = (in_buf[off + 0] & 0xff) as i32;
                                            let au0 = (in_buf[off + 0] >> 8) as i32;
                                            let ay1 = (in_buf[off + 1] & 0xff) as i32;
                                            let av0 = (in_buf[off + 1] >> 8) as i32;
                                            
                                            let by0 = (in_buf[off + 2] & 0xff) as i32;
                                            let bu0 = (in_buf[off + 2] >> 8) as i32;
                                            let by1 = (in_buf[off + 3] & 0xff) as i32;
                                            let bv0 = (in_buf[off + 3] >> 8) as i32;
                                            
                                            let cy0 = (in_buf[off + 4] & 0xff) as i32;
                                            let cu0 = (in_buf[off + 4] >> 8) as i32;
                                            let cy1 = (in_buf[off + 5] & 0xff) as i32;
                                            let cv0 = (in_buf[off + 5] >> 8) as i32;
                                            
                                            let dy0 = (in_buf[off + 6] & 0xff) as i32;
                                            let du0 = (in_buf[off + 6] >> 8) as i32;
                                            let dy1 = (in_buf[off + 7] & 0xff) as i32;
                                            let dv0 = (in_buf[off + 7] >> 8) as i32;
                                            
                                            let abcd1 = convert_ycrcb_to_rgba_sse2(
                                                _mm_set_epi32(ay0, by0, cy0, dy0),
                                                _mm_set_epi32(av0, bv0, cv0, dv0),
                                                _mm_set_epi32(au0, bu0, cu0, du0)
                                            );
                                            
                                            let abcd2 = convert_ycrcb_to_rgba_sse2(
                                                _mm_set_epi32(ay1, by1, cy1, dy1),
                                                _mm_set_epi32(av0, bv0, cv0, dv0),
                                                _mm_set_epi32(au0, bu0, cu0, du0)
                                            );
                                            out_u32[off + 0] = _mm_extract_epi32(abcd1, 3) as u32;
                                            out_u32[off + 1] = _mm_extract_epi32(abcd2, 3) as u32;
                                            out_u32[off + 2] = _mm_extract_epi32(abcd1, 2) as u32;
                                            out_u32[off + 3] = _mm_extract_epi32(abcd2, 2) as u32;
                                            out_u32[off + 4] = _mm_extract_epi32(abcd1, 1) as u32;
                                            out_u32[off + 5] = _mm_extract_epi32(abcd2, 1) as u32;
                                            out_u32[off + 6] = _mm_extract_epi32(abcd1, 0) as u32;
                                            out_u32[off + 7] = _mm_extract_epi32(abcd2, 0) as u32;
                                        }
                                    }
                                },
                                CaptureFormat::NV12 => {
                                    // lets convert NV12 to RGB
                                    let in_buf = std::slice::from_raw_parts(scanline0 as *mut u8, (width * height) * 3);
                                    let mut v_off = width * height;
                                    for y in (0..height).step_by(2) {
                                        for x in (0..width).step_by(2) {
                                            let off = y * width + x;
                                            let y0 = in_buf[off] as i32;
                                            let y1 = in_buf[off + 1] as i32;
                                            let y2 = in_buf[off + width] as i32;
                                            let y3 = in_buf[off + width + 1] as i32;
                                            let u = in_buf[v_off] as i32;
                                            let v = in_buf[v_off + 1] as i32;
                                            v_off += 2;
                                            let abcd1 = convert_ycrcb_to_rgba_sse2(
                                                _mm_set_epi32(y0, y1, y2, y3),
                                                _mm_set_epi32(v, v, v, v),
                                                _mm_set_epi32(u, u, u, u)
                                            );
                                            
                                            out_u32[off] = _mm_extract_epi32(abcd1, 3) as u32;
                                            out_u32[off + 1] = _mm_extract_epi32(abcd1, 2) as u32;
                                            out_u32[off + width] = _mm_extract_epi32(abcd1, 1) as u32;
                                            out_u32[off + width + 1] = _mm_extract_epi32(abcd1, 0) as u32;
                                        }
                                    }
                                },
                            }
                            capture_platform.cxthread.unlock_mapped_texture(&capture_platform.texture)
                        }
                        Cx::send_signal(capture_platform.frame_signal, 0);
                    };
                }
            }
        }
        
        check("ReadSample", capture_platform.reader.ReadSample(
            MF_SOURCE_READER_FIRST_VIDEO_STREAM,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut()
        ));
        
        return S_OK
    }
    
    unsafe fn on_flush(&self, _dwStreamIndex: DWORD,) -> HRESULT {
        println!("GOT FLUSH!");
        return S_OK
    }
    
    unsafe fn on_event(&self, _dwStreamIndex: DWORD, _pEvent: *const IMFMediaEvent,) -> HRESULT {
        println!("GOT EVENT!");
        return S_OK
    }
}
/*
fn convert_ycrcb_to_rgba(y: u16, cr: u16, cb: u16) -> u32 {
    fn clip(a: i32) -> u32 {
        if a< 0 {
            return 0
        }
        if a > 255 {
            return 255
        }
        return a as u32
    }
    
    let c = y as i32 - 16;
    let d = cb as i32 - 128;
    let e = cr as i32 - 128;
    
    return (clip((298 * c + 516 * d + 128) >> 8) << 16)
        | (clip((298 * c - 100 * d - 208 * e + 128) >> 8) << 8)
        | (clip((298 * c + 409 * e + 128) >> 8) << 0)
        | (255 << 24);
}*/

unsafe fn convert_ycrcb_to_rgba_sse2(y: __m128i, cr: __m128i, cb: __m128i) -> __m128i {
    
    unsafe fn clip(a: __m128i) -> __m128i {
        _mm_srl_epi32(_mm_max_epi32(_mm_min_epi32(a, _mm_set1_epi32(65535)), _mm_set1_epi32(0)), _mm_setr_epi32(8, 0, 0, 0))
    }
    
    let c = _mm_add_epi32(y, _mm_set1_epi32(-16));
    let d = _mm_add_epi32(cb, _mm_set1_epi32(-128));
    let e = _mm_add_epi32(cr, _mm_set1_epi32(-128));
    
    // red
    let m128 = _mm_set1_epi32(128);
    let mc = _mm_mullo_epi32(c, _mm_set1_epi32(298));
    let r = _mm_add_epi32(_mm_add_epi32(mc, _mm_mullo_epi32(d, _mm_set1_epi32(516))), m128);
    let g = _mm_add_epi32(_mm_add_epi32(_mm_add_epi32(mc, _mm_mullo_epi32(d, _mm_set1_epi32(-100))), _mm_mullo_epi32(e, _mm_set1_epi32(-208))), m128);
    let b = _mm_add_epi32(_mm_add_epi32(mc, _mm_mullo_epi32(e, _mm_set1_epi32(409))), m128);
    _mm_or_si128(
        _mm_or_si128(
            _mm_sll_epi32(clip(r), _mm_setr_epi32(16, 0, 0, 0)),
            _mm_sll_epi32(clip(g), _mm_setr_epi32(8, 0, 0, 0))
        ),
        clip(b)
    )
}

fn check(msg: &str, hr: HRESULT) {
    if winerror::SUCCEEDED(hr) {
        return
    }
    else {
        panic!("{}", msg)
    }
}

fn guid_equal(a: &GUID, b: &GUID) -> bool {
    a.Data1 == b.Data1 && a.Data2 == b.Data2 && a.Data3 == b.Data3 && a.Data4 == b.Data4
}



// Missing windows media foundation bits from winapi



ENUM! {enum MF_ATTRIBUTE_TYPE {
    MF_ATTRIBUTE_UINT32 = VT_UI4,
    MF_ATTRIBUTE_UINT64 = VT_UI8,
    MF_ATTRIBUTE_DOUBLE = VT_R8,
    MF_ATTRIBUTE_GUID = VT_CLSID,
    MF_ATTRIBUTE_STRING = VT_LPWSTR,
    MF_ATTRIBUTE_BLOB = (VT_VECTOR | VT_UI1),
    MF_ATTRIBUTE_IUNKNOWN = VT_UNKNOWN,
}}

ENUM! {enum MF_ATTRIBUTES_MATCH_TYPE {
    MF_ATTRIBUTES_MATCH_OUR_ITEMS = 0,
    MF_ATTRIBUTES_MATCH_THEIR_ITEMS = 1,
    MF_ATTRIBUTES_MATCH_ALL_ITEMS = 2,
    MF_ATTRIBUTES_MATCH_INTERSECTION = 3,
    MF_ATTRIBUTES_MATCH_SMALLER = 4,
}}

ENUM! {enum MediaEventType {
    MAKE_DWORD = 0u32,
}}

const MF_SOURCE_READER_FIRST_VIDEO_STREAM: DWORD = 0xfffffffc;

DEFINE_GUID! {MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, 0xc60ac5fe, 0x252a, 0x478f, 0xa0, 0xef, 0xbc, 0x8f, 0xa5, 0xf7, 0xca, 0xd3}
DEFINE_GUID! {MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, 0x8ac3587a, 0x4ae7, 0x42d8, 0x99, 0xe0, 0x0a, 0x60, 0x13, 0xee, 0xf9, 0x0f}
DEFINE_GUID! {MF_READWRITE_DISABLE_CONVERTERS, 0x98d5b065, 0x1374, 0x4847, 0x8d, 0x5d, 0x31, 0x52, 0x0f, 0xee, 0x71, 0x56}
DEFINE_GUID! {MF_SOURCE_READER_ASYNC_CALLBACK, 0x1e3dbeac, 0xbb43, 0x4c35, 0xb5, 0x07, 0xcd, 0x64, 0x44, 0x64, 0xc9, 0x65}
DEFINE_GUID! {MF_MT_SUBTYPE, 0xf7e34c9a, 0x42e8, 0x4714, 0xb7, 0x4b, 0xcb, 0x29, 0xd7, 0x2c, 0x35, 0xe5}
DEFINE_GUID! {MF_MT_FRAME_SIZE, 0x1652c33d, 0xd6b2, 0x4012, 0xb8, 0x34, 0x72, 0x03, 0x08, 0x49, 0xa3, 0x7d}
DEFINE_GUID! {MF_MT_FRAME_RATE, 0xc459a2e8, 0x3d2c, 0x4e44, 0xb1, 0x32, 0xfe, 0xe5, 0x15, 0x6c, 0x7b, 0xb0}

DEFINE_GUID! {MFVideoFormat_RGB32, 22, 0x0000, 0x0010, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}
DEFINE_GUID! {MFVideoFormat_RGB24, 20, 0x0000, 0x0010, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}
DEFINE_GUID! {MFVideoFormat_YUY2, 0x32595559, 0x0000, 0x0010, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}
DEFINE_GUID! {MFVideoFormat_NV12, 0x3231564e, 0x0000, 0x0010, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}

RIDL! {
    #[uuid(0x2cd2d921, 0xc447, 0x44a7, 0xa1, 0x3c, 0x4a, 0xda, 0xbf, 0xc2, 0x47, 0xe3)]
    interface IMFAttributes(IMFAttributesVtbl): IUnknown(IUnknownVtbl) {
        
        fn GetItem(guidKey: REFGUID, pValue: *mut PROPVARIANT,) -> HRESULT,
        fn GetItemType(guidKey: REFGUID, pType: *mut MF_ATTRIBUTE_TYPE,) -> HRESULT,
        fn CompareItem(guidKey: REFGUID, Value: REFPROPVARIANT, pbResult: *mut BOOL,) -> HRESULT,
        fn Compare(pTheirs: *mut IMFAttributes, MatchType: MF_ATTRIBUTES_MATCH_TYPE, pbResult: *mut BOOL,) -> HRESULT,
        fn GetUINT32(guidKey: REFGUID, punValue: *mut u32,) -> HRESULT,
        fn GetUINT64(guidKey: REFGUID, punValue: *mut u64,) -> HRESULT,
        fn GetDouble(guidKey: REFGUID, pfValue: *mut f64,) -> HRESULT,
        fn GetGUID(guidKey: REFGUID, pguidValue: *mut GUID,) -> HRESULT,
        fn GetStringLength(guidKey: REFGUID, pcchLength: *mut u32,) -> HRESULT,
        fn GetString(guidKey: REFGUID, pwszValue: LPWSTR, cchBufSize: u32, pcchLength: *mut u32,) -> HRESULT,
        fn GetAllocatedString(guidKey: REFGUID, pwszValue: *mut LPWSTR, pcchLength: &mut u32,) -> HRESULT,
        fn GetBlobSize(guidKey: REFGUID, pcbBlobSize: *mut u32,) -> HRESULT,
        fn GetBlob(guidKey: REFGUID, pBuf: *mut u8, cbBufSize: u32, pcbBlobSize: *mut u32,) -> HRESULT,
        fn GetAllocatedBlob(guidKey: REFGUID, ppBuf: *mut *mut u8, pcbSize: *mut u32,) -> HRESULT,
        fn GetUnknown(guidKey: REFGUID, riid: REFIID, ppv: *mut *mut c_void,) -> HRESULT,
        fn SetItem(guidKey: REFGUID, Value: REFPROPVARIANT,) -> HRESULT,
        fn DeleteItem(guidKey: REFGUID,) -> HRESULT,
        fn DeleteAllItems() -> HRESULT,
        fn SetUINT32(guidKey: REFGUID, unValue: u32,) -> HRESULT,
        fn SetUINT64(guidKey: REFGUID, unValue: u64,) -> HRESULT,
        fn SetDouble(guidKey: REFGUID, fValue: f64,) -> HRESULT,
        fn SetGUID(guidKey: REFGUID, guidValue: REFGUID,) -> HRESULT,
        fn SetString(guidKey: REFGUID, wszValue: LPCWSTR,) -> HRESULT,
        fn SetBlob(guidKey: REFGUID, pBuf: *const u8, cbBufSize: u32,) -> HRESULT,
        fn SetUnknown(guidKey: REFGUID, pUnknown: *const IUnknown,) -> HRESULT,
        fn LockStore() -> HRESULT,
        fn UnlockStore() -> HRESULT,
        fn GetCount(pcItems: *mut u32,) -> HRESULT,
        fn GetItemByIndex(unIndex: u32, pguidKey: *mut GUID, pValue: *mut PROPVARIANT,) -> HRESULT,
        fn CopyAllItems(pDest: *const IMFAttributes,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x7FEE9E9A, 0x4A89, 0x47a6, 0x89, 0x9c, 0xb6, 0xa5, 0x3a, 0x70, 0xfb, 0x67)]
    interface IMFActivate(IMFActivateVtbl): IMFAttributes(IMFAttributesVtbl) {
        fn ActivateObject(riid: REFIID, ppv: *mut *mut c_void,) -> HRESULT,
        fn ShutdownObject() -> HRESULT,
        fn DetachObject() -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x44ae0fa8, 0xea31, 0x4109, 0x8d, 0x2e, 0x4c, 0xae, 0x49, 0x97, 0xc5, 0x55)]
    interface IMFMediaType(IMFMediaTypeVtbl): IMFAttributes(IMFAttributesVtbl) {
        fn GetMajorType(pguidMajorType: *mut GUID,) -> HRESULT,
        fn IsCompressedFormat(pfCompressed: *mut BOOL,) -> HRESULT,
        fn IsEqual(pIMediaType: *const IMFMediaType, pdwFlags: *mut DWORD,) -> HRESULT,
        fn GetRepresentation(guidRepresentation: GUID, ppvRepresentation: *mut *mut c_void,) -> HRESULT,
        fn FreeRepresentation(guidRepresentation: GUID, pvRepresentation: *mut c_void,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x2CD0BD52, 0xBCD5, 0x4B89, 0xb6, 0x2c, 0xea, 0xdc, 0x0c, 0x03, 0x1e, 0x7d)]
    interface IMFMediaEventGenerator(IMFMediaEventGeneratorVtbl): IUnknown(IUnknownVtbl) {
        fn GetEvent(dwFlags: DWORD, ppEvent: *mut *mut IMFMediaEvent,) -> HRESULT,
        fn BeginGetEvent(pCallback: *const IMFAsyncCallback, punkState: *const IUnknown,) -> HRESULT,
        fn EndGetEvent(pResult: *const IMFAsyncResult, ppEvent: *mut *mut IMFMediaEvent,) -> HRESULT,
        fn QueueEvent(met: MediaEventType, guidExtendedType: REFGUID, hrStatus: HRESULT, pvValue: *const PROPVARIANT,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0xDF598932, 0xF10C, 0x4E39, 0xbb, 0xa2, 0xc3, 0x08, 0xf1, 0x01, 0xda, 0xa3)]
    interface IMFMediaEvent(IMFMediaEventVtbl): IMFAttributes(IMFAttributesVtbl) {
        fn GetType(pmet: *mut MediaEventType,) -> HRESULT,
        fn GetExtendedType(pguidExtendedType: *mut GUID,) -> HRESULT,
        fn GetStatus(phrStatus: *mut HRESULT,) -> HRESULT,
        fn GetValue(pvValue: *mut PROPVARIANT,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x279a808d, 0xaec7, 0x40c8, 0x9c, 0x6b, 0xa6, 0xb4, 0x92, 0xc7, 0x8a, 0x66)]
    interface IMFMediaSource(IMFMediaSourceVtbl): IMFMediaEventGenerator(IMFMediaEventGeneratorVtbl) {
        fn GetCharacteristics(pdwCharacteristics: *mut DWORD,) -> HRESULT,
        fn CreatePresentationDescriptor(ppPresentationDescriptor: *mut *mut IMFPresentationDescriptor,) -> HRESULT,
        fn Start(
            pPresentationDescriptor: *const IMFPresentationDescriptor,
            pguidTimeFormat: *const GUID,
            pvarStartPosition: *const PROPVARIANT,
        ) -> HRESULT,
        fn Stop() -> HRESULT,
        fn Pause() -> HRESULT,
        fn Shutdown() -> HRESULT,
    }
}

RIDL! {
    #[uuid(0xa27003cf, 0x2354, 0x4f2a, 0x8d, 0x6a, 0xab, 0x7c, 0xff, 0x15, 0x43, 0x7e)]
    interface IMFAsyncCallback(IMFAsyncCallbackVtbl): IUnknown(IUnknownVtbl) {
        fn GetParameters(pdwFlags: *mut DWORD, pdwQueue: *mut DWORD,) -> HRESULT,
        fn Invoke(pAsyncResult: *const IMFAsyncResult,) -> HRESULT,
    }
}


RIDL! {
    #[uuid(0xac6b7889, 0x0740, 0x4d51, 0x86, 0x19, 0x90, 0x59, 0x94, 0xa5, 0x5c, 0xc6)]
    interface IMFAsyncResult(IMFAsyncResultVtbl): IUnknown(IUnknownVtbl) {
        fn GetState(ppunkState: *mut *mut IUnknown,) -> HRESULT,
        fn GetStatus() -> HRESULT,
        fn SetStatus(hrStatus: HRESULT,) -> HRESULT,
        fn GetObject(ppObject: *mut *mut IUnknown,) -> HRESULT,
        fn GetStateNoAddRef() -> *mut IUnknown,
    }
}

RIDL! {
    #[uuid(0x03cb2711, 0x24d7, 0x4db6, 0xa1, 0x7f, 0xf3, 0xa7, 0xa4, 0x79, 0xa5, 0x36)]
    interface IMFPresentationDescriptor(IMFPresentationDescriptorVtbl): IMFAttributes(IMFAttributesVtbl) {
        fn GetStreamDescriptorCount(pdwDescriptorCount: *mut DWORD,) -> HRESULT,
        fn GetStreamDescriptorByIndex(dwIndex: DWORD, pfSelected: *mut BOOL, ppDescriptor: *mut *mut IMFStreamDescriptor,) -> HRESULT,
        fn SelectStream(dwDescriptorIndex: DWORD,) -> HRESULT,
        fn DeselectStream(dwDescriptorIndex: DWORD,) -> HRESULT,
        fn Clone(ppPresentationDescriptor: *mut *mut IMFPresentationDescriptor,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x56c03d9c, 0x9dbb, 0x45f5, 0xab, 0x4b, 0xd8, 0x0f, 0x47, 0xc0, 0x59, 0x38)]
    interface IMFStreamDescriptor(IMFStreamDescriptorVtbl): IMFAttributes(IMFAttributesVtbl) {
        fn GetStreamIdentifier(pdwStreamIdentifier: *mut DWORD,) -> HRESULT,
        fn GetMediaTypeHandler(ppMediaTypeHandler: *mut *mut IMFMediaTypeHandler,) -> HRESULT,
    }
}


RIDL! {
    #[uuid(0xe93dcf6c, 0x4b07, 0x4e1e, 0x81, 0x23, 0xaa, 0x16, 0xed, 0x6e, 0xad, 0xf5)]
    interface IMFMediaTypeHandler(IMFMediaTypeHandlerVtbl): IUnknown(IUnknownVtbl) {
        fn IsMediaTypeSupported(pMediaType: *const IMFMediaType, ppMediaType: *mut *mut IMFMediaType,) -> HRESULT,
        fn GetMediaTypeCount(pdwTypeCount: *mut DWORD,) -> HRESULT,
        fn GetMediaTypeByIndex(dwIndex: DWORD, ppType: *mut *mut IMFMediaType,) -> HRESULT,
        fn SetCurrentMediaType(pMediaType: *const IMFMediaType,) -> HRESULT,
        fn GetCurrentMediaType(ppMediaType: *mut *mut IMFMediaType,) -> HRESULT,
        fn GetMajorType(pguidMajorType: *mut GUID,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0xdeec8d99, 0xfa1d, 0x4d82, 0x84, 0xc2, 0x2c, 0x89, 0x69, 0x94, 0x48, 0x67)]
    interface IMFSourceReaderCallback(IMFSourceReaderCallbackVtbl): IUnknown(IUnknownVtbl) {
        fn OnReadSample(
            hrState: HRESULT,
            dwStreamIndex: DWORD,
            dwStreamFlags: DWORD,
            llTimeStamp: LONGLONG,
            pSample: *const IMFSample,
        ) -> HRESULT,
        fn OnFlush(dwStreamIndex: DWORD,) -> HRESULT,
        fn OnEvent(dwStreamIndex: DWORD, pEvent: *const IMFMediaEvent,) -> HRESULT,
    }
}


RIDL! {
    #[uuid(0xc40a00f2, 0xb93a, 0x4d80, 0xae, 0x8c, 0x5a, 0x1c, 0x63, 0x4f, 0x58, 0xe4)]
    interface IMFSample(IMFSampleVtbl): IMFAttributes(IMFAttributesVtbl) {
        fn GetSampleFlags(pdwSampleFlags: *mut DWORD,) -> HRESULT,
        fn SetSampleFlags(dwSampleFlags: DWORD,) -> HRESULT,
        fn GetSampleTime(phnsSampleTime: *mut LONGLONG,) -> HRESULT,
        fn SetSampleTime(hnsSampleTime: LONGLONG,) -> HRESULT,
        fn GetSampleDuration(phnsSampleDuration: *mut LONGLONG,) -> HRESULT,
        fn SetSampleDuration(hnsSampleDuration: LONGLONG,) -> HRESULT,
        fn GetBufferCount(pdwBufferCount: *mut DWORD,) -> HRESULT,
        fn GetBufferByIndex(dwIndex: DWORD, ppBuffer: *mut *mut IMFMediaBuffer,) -> HRESULT,
        fn ConvertToContiguousBuffer(ppBuffer: *mut *mut IMFMediaBuffer,) -> HRESULT,
        fn AddBuffer(pBuffer: *const IMFMediaBuffer,) -> HRESULT,
        fn RemoveBufferByIndex(dwIndex: DWORD,) -> HRESULT,
        fn RemoveAllBuffers() -> HRESULT,
        fn GetTotalLength(pcbTotalLength: *mut DWORD,) -> HRESULT,
        fn CopyToBuffer(pBuffer: *const IMFMediaBuffer,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x045FA593, 0x8799, 0x42b8, 0xbc, 0x8d, 0x89, 0x68, 0xc6, 0x45, 0x35, 0x07)]
    interface IMFMediaBuffer(IMFMediaBufferVtbl): IUnknown(IUnknownVtbl) {
        fn Lock(ppbBuffer: *mut *mut BYTE, pcbMaxLength: *mut DWORD, pcbCurrentLength: *mut DWORD,) -> HRESULT,
        fn Unlock() -> HRESULT,
        fn GetCurrentLength(pcbCurrentLength: *mut DWORD,) -> HRESULT,
        fn GetMaxLength(pcbMaxLength: *mut DWORD,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x7DC9D5F9, 0x9ED9, 0x44ec, 0x9b, 0xbf, 0x06, 0x00, 0xbb, 0x58, 0x9f, 0xbb)]
    interface IMF2DBuffer(IMF2DBufferVtbl): IUnknown(IUnknownVtbl) {
        fn Lock2D(ppbScanline0: *mut *mut BYTE, plPitch: *mut LONG,) -> HRESULT,
        fn Unlock2D() -> HRESULT,
        fn GetScanline0AndPitch(ppbScanline0: *mut *mut BYTE, plPitch: *mut LONG,) -> HRESULT,
        fn IsContiguousFormat(pfIsContiguous: *mut BOOL,) -> HRESULT,
        fn GetContiguousLength(pcbLength: *mut DWORD,) -> HRESULT,
        fn ContiguousCopyTo(pbDestBuffer: *mut BYTE, cbDestBuffer: DWORD,) -> HRESULT,
        fn ContiguousCopyFrom(pbSrcBuffer: *const BYTE, cbSrcBuffer: DWORD,) -> HRESULT,
    }
}

RIDL! {
    #[uuid(0x70ae66f2, 0xc809, 0x4e4f, 0x89, 0x15, 0xbd, 0xcb, 0x40, 0x6b, 0x79, 0x93)]
    interface IMFSourceReader(IMFSourceReaderVtbl): IUnknown(IUnknownVtbl) {
        fn GetStreamSelection(dwStreamIndex: DWORD, pfSelected: *mut BOOL,) -> HRESULT,
        fn SetStreamSelection(dwStreamIndex: DWORD, fSelected: BOOL,) -> HRESULT,
        fn GetNativeMediaType(dwStreamIndex: DWORD, dwMediaTypeIndex: DWORD, ppMediaType: *mut *mut IMFMediaType,) -> HRESULT,
        fn GetCurrentMediaType(dwStreamIndex: DWORD, ppMediaType: *mut *mut IMFMediaType,) -> HRESULT,
        fn SetCurrentMediaType(dwStreamIndex: DWORD, pdwReserved: *const DWORD, pMediaType: *mut IMFMediaType,) -> HRESULT,
        fn SetCurrentPosition(guidTimeFormat: REFGUID, varPosition: REFPROPVARIANT,) -> HRESULT,
        fn ReadSample(
            dwStreamIndex: DWORD,
            dwControlFlags: DWORD,
            pdwActualStreamIndex: *mut DWORD,
            pdwStreamFlags: *mut DWORD,
            pllTimestamp: *mut LONGLONG,
            ppSample: *mut *mut IMFSample,
        ) -> HRESULT,
        fn Flush(dwStreamIndex: DWORD,) -> HRESULT,
        fn GetServiceForStream(
            dwStreamIndex: DWORD,
            guidService: REFGUID,
            riid: REFIID,
            ppvObject: *mut *mut c_void,
        ) -> HRESULT,
        fn GetPresentationAttribute(
            dwStreamIndex: DWORD,
            guidAttribute: REFGUID,
            pvarAttribute: *mut PROPVARIANT,
        ) -> HRESULT,
    }
}

#[cfg(target_env = "gnu")]
#[link(name = "winapi_mfplat")]
extern "system" {
    pub fn MFStartup(version: UINT) -> HRESULT;
    pub fn MFCreateAttributes(
        ppMFAttributes: *mut *mut IMFAttributes,
        cInitialSize: u32
    ) -> HRESULT;
}

#[cfg(target_env = "msvc")]
#[link(name = "mfplat")]
extern "system" {
    pub fn MFStartup(version: UINT) -> HRESULT;
    pub fn MFCreateAttributes(
        ppMFAttributes: *mut *mut IMFAttributes,
        cInitialSize: u32
    ) -> HRESULT;
}


#[cfg(target_env = "gnu")]
#[link(name = "winapi_mfcore")]
extern "system" {
    pub fn MFEnumDeviceSources(
        pAttributes: *const IMFAttributes,
        pppSourceActivate: *mut *mut *mut IMFActivate,
        pcSourceActivate: *mut u32
    ) -> HRESULT;
}

#[cfg(target_env = "msvc")]
#[link(name = "mfcore")]
extern "system" {
    pub fn MFEnumDeviceSources(
        pAttributes: *const IMFAttributes,
        pppSourceActivate: *mut *mut *mut IMFActivate,
        pcSourceActivate: *mut u32
    ) -> HRESULT;
}

#[cfg(target_env = "gnu")]
#[link(name = "winapi_mfreadwrite")]
extern "system" {
    pub fn MFCreateSourceReaderFromMediaSource(
        pMediaSource: *const IMFMediaSource,
        pAttributes: *const IMFAttributes,
        ppSourceReader: *mut *mut IMFSourceReader
    ) -> HRESULT;
}

#[cfg(target_env = "msvc")]
#[link(name = "mfreadwrite")]
extern "system" {
    pub fn MFCreateSourceReaderFromMediaSource(
        pMediaSource: *const IMFMediaSource,
        pAttributes: *const IMFAttributes,
        ppSourceReader: *mut *mut IMFSourceReader
    ) -> HRESULT;
}
