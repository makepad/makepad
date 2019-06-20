#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(dead_code)]
#![allow(unused_variables)]

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
use winapi::um::winnt::{HRESULT, LPCWSTR, LONGLONG, LPWSTR};
use winapi::shared::winerror::{S_OK};
use winapi::shared::winerror;
use winapi::shared::guiddef::{GUID, REFIID, REFGUID};
use winapi::shared::wtypes::{VT_UI4, VT_UI8, VT_R8, VT_CLSID, VT_LPWSTR, VT_VECTOR, VT_UI1, VT_UNKNOWN};
use std::ptr;
use com_impl::{Refcount, VTable};

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

DEFINE_GUID! {MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, 0xc60ac5fe, 0x252a, 0x478f, 0xa0, 0xef, 0xbc, 0x8f, 0xa5, 0xf7, 0xca, 0xd3}
DEFINE_GUID! {MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID, 0x8ac3587a, 0x4ae7, 0x42d8, 0x99, 0xe0, 0x0a, 0x60, 0x13, 0xee, 0xf9, 0x0f}
DEFINE_GUID! {MF_READWRITE_DISABLE_CONVERTERS, 0x98d5b065, 0x1374, 0x4847, 0x8d, 0x5d, 0x31, 0x52, 0x0f, 0xee, 0x71, 0x56}
DEFINE_GUID! {MF_SOURCE_READER_ASYNC_CALLBACK, 0x1e3dbeac, 0xbb43, 0x4c35, 0xb5, 0x07, 0xcd, 0x64, 0x44, 0x64, 0xc9, 0x65}

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


#[link(name = "winapi_mfplat")]extern "system" {
    pub fn MFStartup(version: UINT) -> HRESULT;
    pub fn MFCreateAttributes(
        ppMFAttributes: *mut *mut IMFAttributes,
        cInitialSize: u32
    ) -> HRESULT;
}

#[link(name = "winapi_mfcore")]extern "system" {
    pub fn MFEnumDeviceSources(
        pAttributes: *const IMFAttributes,
        pppSourceActivate: *mut *mut *mut IMFActivate,
        pcSourceActivate: *mut u32
    ) -> HRESULT;
}


#[repr(C)]
#[derive(com_impl::ComImpl)]
pub struct SourceReader {
    vtbl: VTable<IMFSourceReaderCallbackVtbl>,
    refcount: Refcount,
}

impl SourceReader {
    // Todo: Use a wrapper type for the ComPtr
    pub fn new() -> ComPtr<IMFSourceReaderCallback> {
        let ptr = SourceReader::create_raw();
        let ptr = ptr as *mut IMFSourceReaderCallback;
        unsafe {ComPtr::from_raw(ptr)}
    }
}


#[com_impl::com_impl]
unsafe impl IMFSourceReaderCallback for SourceReader {
    unsafe fn on_read_sample(&self,
        hrState: HRESULT,
        dwStreamIndex: DWORD,
        dwStreamFlags: DWORD,
        llTimeStamp: LONGLONG,
        pSample: *const IMFSample,
    ) -> HRESULT {
        return S_OK
    }
    
    unsafe fn on_flush(&self, dwStreamIndex: DWORD,) -> HRESULT {
        return S_OK
    }

    unsafe fn on_event(&self, dwStreamIndex: DWORD, pEvent: *const IMFMediaEvent,) -> HRESULT {
        return S_OK
    }
}

#[derive(Clone, Default)]
pub struct CapWinMF {
}

fn check(msg: &str, hr: HRESULT) {
    if winerror::SUCCEEDED(hr) {
        return
    }
    else {
        panic!("{}", msg)
    }
}

impl CapWinMF {
    
    pub fn mf_create_attributes(num_attrs: DWORD) -> Result<ComPtr<IMFAttributes>, HRESULT> {
        let mut attributes = ptr::null_mut();
        let hr = unsafe {MFCreateAttributes(&mut attributes as *mut *mut _, num_attrs)};
        if winerror::SUCCEEDED(hr) {Ok(unsafe {ComPtr::from_raw(attributes as *mut _)})} else {Err(hr)}
    }
    
    pub fn init(&mut self, cx: &mut Cx) {
        
        check("cannot mf_startup", unsafe {
            MFStartup(0)
        });
        
        let attributes = Self::mf_create_attributes(1).expect("cannot mf_create_attributes");
        
        check("attributes.SetGUID", unsafe {
            attributes.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)
        });
        
        
        unsafe {
            let mut devices_raw: *mut *mut IMFActivate = 0 as _;
            let mut count: u32 = 0;
            check("attributes.SetGUID", MFEnumDeviceSources(attributes.as_raw() as *mut _, &mut devices_raw, &mut count));
            
            let devices = std::slice::from_raw_parts(devices_raw, count as usize);
            
            let mut source_raw = ptr::null_mut();
            check("ActivateObject", (*devices[0]).ActivateObject(&IMFMediaSource::uuidof(), &mut source_raw));
            
            let source: ComPtr<IMFMediaSource> = ComPtr::from_raw(source_raw as *mut _);
            
            let attributes = Self::mf_create_attributes(3).expect("cannot mf_create_attributes");
            
            check("attributes.SetUINT32",
                attributes.SetUINT32(&MF_READWRITE_DISABLE_CONVERTERS, TRUE as u32)
            );
            
            let source_reader = SourceReader::new();
            check("attributes.SetUINT32", 
                attributes.SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, source_reader.as_raw() as * mut _)
            );

            // set reader async callback
            println!("MADE IT HERE");
            
        };
        
        
    }
}/*
    device_manager: Option<ComPtr<IMFDXGIDeviceManager>>
    pub fn create_device_manager(cx: &mut Cx)
        -> Result<(UINT, ComPtr<IMFDXGIDeviceManager>), HRESULT> {
        let mut device_manager = ptr::null_mut();
        let mut reset_token: UINT = 0;
        let hr = unsafe {MFCreateDXGIDeviceManager(
            &mut reset_token,
            &mut device_manager as *mut *mut _
        )};
        if winerror::SUCCEEDED(hr) {
            Ok((reset_token, unsafe {ComPtr::from_raw(device_manager as *mut _)}))
        }
        else {
            Err(hr)
        }
    }
    
    pub fn reset_device(cx: &mut Cx, reset_token: UINT, device_manager: ComPtr<IMFDXGIDeviceManager>)
        -> Result<(), HRESULT> {
        let d3d11_cx = unsafe {&(*cx.platform.d3d11_cx.unwrap())};
        let hr = unsafe {device_manager.ResetDevice(
            d3d11_cx.device.as_raw() as *mut _,
            reset_token,
        )};
        if winerror::SUCCEEDED(hr) {
            Ok(())
        }
        else {
            Err(hr)
        }
    }
    
RIDL!{#[uuid(0xeb533d5d, 0x2db6, 0x40f8, 0x97, 0xa9, 0x49, 0x46, 0x92, 0x01, 0x4f, 0x07)]interface IMFDXGIDeviceManager(IMFDXGIDeviceManagerVtbl): IUnknown(IUnknownVtbl) {
    fn CloseDeviceHandle(hDevice: HANDLE,) -> HRESULT,
    fn GetVideoService(hDevice: HANDLE, riid: REFIID, ppService: *mut *mut c_void,) -> HRESULT,
    fn LockDevice(hDevice: HANDLE, riid: REFIID, ppUnkDevice: *mut *mut c_void, fBlock: BOOL,) -> HRESULT,
    fn OpenDeviceHandle(phDevice: *mut HANDLE,) -> HRESULT,
    fn ResetDevice(pUnkDevice: *const IUnknown, resetToken: UINT,) -> HRESULT,
    fn TestDevice(hDevice: HANDLE,) -> HRESULT,
    fn UnlockDevice(hDevice: HANDLE, fSaveState: BOOL,) -> HRESULT,
}}

#[link(name = "winapi_mfplat")]
extern "system" {
    pub fn MFCreateDXGIDeviceManager(
        resetToken: *mut UINT,
        ppDeviceManager: *mut *mut IMFDXGIDeviceManager,
    ) -> HRESULT;
}
*/