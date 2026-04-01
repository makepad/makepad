#[cfg(not(target_os = "macos"))]
mod imp {
    use crate::backend::{BackendCapabilities, BackendInfo, BackendKind};

    pub type MetalResult<T> = Result<T, String>;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct MetalSize {
        pub width: u64,
        pub height: u64,
        pub depth: u64,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct MetalBufferBindingRef<'a> {
        pub index: u64,
        pub buffer: &'a MetalBuffer,
        pub offset_bytes: usize,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub enum BufferStorageMode {
        Shared,
        Private,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub enum FunctionConstantValue {
        Int32(i32),
        Int16(i16),
        Bool(bool),
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct FunctionConstant {
        pub idx: i32,
        pub value: FunctionConstantValue,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct MetalPipelineDescriptor {
        pub cache_name: String,
        pub base_name: String,
        pub constants: Vec<FunctionConstant>,
        pub smem_bytes: usize,
        pub nr0: i32,
        pub nr1: i32,
        pub nsg: i32,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct MetalDeviceFeatures {
        pub has_bfloat: bool,
        pub has_tensor: bool,
        pub has_simdgroup_mm: bool,
    }

    #[derive(Clone, Debug)]
    pub struct MetalBuffer {
        size_bytes: usize,
        storage: BufferStorageMode,
    }

    impl MetalBuffer {
        pub fn size_bytes(&self) -> usize {
            self.size_bytes
        }

        pub fn storage(&self) -> BufferStorageMode {
            self.storage
        }
    }

    #[derive(Clone, Debug)]
    pub struct MetalPipeline {
        pub smem_bytes: usize,
        pub nr0: i32,
        pub nr1: i32,
        pub nsg: i32,
        pub max_threads_per_threadgroup: u64,
    }

    pub struct MetalRuntime {
        info: BackendInfo,
        features: MetalDeviceFeatures,
    }

    impl MetalRuntime {
        pub fn is_available() -> bool {
            false
        }

        pub fn new() -> MetalResult<Self> {
            Err("Metal runtime is only available on macOS in this port".to_string())
        }

        pub fn backend_info(&self) -> &BackendInfo {
            &self.info
        }

        pub fn features(&self) -> MetalDeviceFeatures {
            self.features
        }

        pub fn create_buffer_with_bytes(
            &self,
            _bytes: &[u8],
            _storage: BufferStorageMode,
        ) -> MetalResult<MetalBuffer> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn create_buffer(
            &self,
            _size_bytes: usize,
            _storage: BufferStorageMode,
        ) -> MetalResult<MetalBuffer> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn get_or_compile_pipeline(
            &self,
            _desc: &MetalPipelineDescriptor,
        ) -> MetalResult<MetalPipeline> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn read_buffer(
            &self,
            _buffer: &MetalBuffer,
            _len_bytes: usize,
        ) -> MetalResult<Vec<u8>> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn read_buffer_range(
            &self,
            _buffer: &MetalBuffer,
            _offset_bytes: usize,
            _len_bytes: usize,
        ) -> MetalResult<Vec<u8>> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn write_buffer(
            &self,
            _buffer: &MetalBuffer,
            _offset_bytes: usize,
            _bytes: &[u8],
        ) -> MetalResult<()> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn dispatch_compute(
            &self,
            _pipeline: &MetalPipeline,
            _args_bytes: &[u8],
            _buffers: &[MetalBufferBindingRef<'_>],
            _threadgroup_memory_lengths: &[(u64, usize)],
            _threadgroups: MetalSize,
            _threads_per_threadgroup: MetalSize,
        ) -> MetalResult<()> {
            Err("Metal runtime is unavailable on this target".to_string())
        }

        pub fn wait_idle(&self) -> MetalResult<()> {
            Err("Metal runtime is unavailable on this target".to_string())
        }
    }

    impl Default for MetalRuntime {
        fn default() -> Self {
            Self {
                info: BackendInfo {
                    kind: BackendKind::Metal,
                    name: "Unavailable".to_string(),
                    description: "Metal runtime stub".to_string(),
                    capabilities: BackendCapabilities::default(),
                },
                features: MetalDeviceFeatures::default(),
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::backend::{BackendCapabilities, BackendInfo, BackendKind};
    use makepad_objc_sys::runtime::{nil, ObjcId, Object, YES};
    use makepad_objc_sys::{class, msg_send, sel, sel_impl};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::{c_char, c_void, CStr};
    use std::ptr::NonNull;

    pub type MetalResult<T> = Result<T, String>;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct MetalSize {
        pub width: u64,
        pub height: u64,
        pub depth: u64,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct MetalBufferBindingRef<'a> {
        pub index: u64,
        pub buffer: &'a MetalBuffer,
        pub offset_bytes: usize,
    }

    const UTF8_ENCODING: u64 = 4;
    const MTL_RESOURCE_STORAGE_MODE_SHARED: u64 = 0;
    const MTL_RESOURCE_OPTIONS_STORAGE_MODE_PRIVATE: u64 = 32;
    const MTL_STORAGE_MODE_PRIVATE: u64 = 2;
    const MTL_GPU_FAMILY_APPLE6: u64 = 1006;
    const MTL_GPU_FAMILY_APPLE7: u64 = 1007;
    const MTL_GPU_FAMILY_METAL3: u64 = 5001;
    const MTL_GPU_FAMILY_METAL4: u64 = 5002;
    const MTL_DATA_TYPE_INT: u64 = 29;
    const MTL_DATA_TYPE_SHORT: u64 = 37;
    const MTL_DATA_TYPE_BOOL: u64 = 53;

    const GGML_METAL_SOURCE_RAW: &str = include_str!("ggml/ggml-metal.metal");
    const GGML_COMMON_H: &str = include_str!("ggml/ggml-common.h");
    const GGML_METAL_IMPL_H: &str = include_str!("ggml/ggml-metal-impl.h");
    const GGML_METALLIB_BYTES: &[u8] = include_bytes!(env!("MAKEPAD_GGML_METALLIB"));

    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> ObjcId;
        fn MTLCopyAllDevices() -> ObjcId;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    #[repr(C)]
    #[derive(Copy, Clone)]
    struct MTLSize {
        width: u64,
        height: u64,
        depth: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub enum BufferStorageMode {
        Shared,
        Private,
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub enum FunctionConstantValue {
        Int32(i32),
        Int16(i16),
        Bool(bool),
    }

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct FunctionConstant {
        pub idx: i32,
        pub value: FunctionConstantValue,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct MetalPipelineDescriptor {
        pub cache_name: String,
        pub base_name: String,
        pub constants: Vec<FunctionConstant>,
        pub smem_bytes: usize,
        pub nr0: i32,
        pub nr1: i32,
        pub nsg: i32,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct MetalDeviceFeatures {
        pub has_bfloat: bool,
        pub has_tensor: bool,
        pub has_simdgroup_mm: bool,
    }

    #[derive(Debug)]
    struct StrongId(NonNull<Object>);

    impl StrongId {
        unsafe fn from_owned(id: ObjcId) -> Option<Self> {
            NonNull::new(id).map(Self)
        }

        unsafe fn from_unowned(id: ObjcId) -> Option<Self> {
            if id.is_null() {
                return None;
            }
            let _: () = msg_send![id, retain];
            NonNull::new(id).map(Self)
        }

        fn as_id(&self) -> ObjcId {
            self.0.as_ptr()
        }
    }

    impl Clone for StrongId {
        fn clone(&self) -> Self {
            unsafe {
                let _: () = msg_send![self.as_id(), retain];
            }
            Self(self.0)
        }
    }

    impl Drop for StrongId {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0.as_ptr(), release];
            }
        }
    }

    struct AutoreleasePool(ObjcId);

    impl AutoreleasePool {
        fn new() -> Self {
            let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
            Self(pool)
        }
    }

    impl Drop for AutoreleasePool {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _: () = msg_send![self.0, release];
                }
            }
        }
    }

    fn nsstring_to_string(ns_string: ObjcId) -> String {
        if ns_string.is_null() {
            return String::new();
        }
        unsafe {
            let utf8_ptr: *const c_char = msg_send![ns_string, UTF8String];
            if utf8_ptr.is_null() {
                return String::new();
            }
            CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned()
        }
    }

    fn str_to_nsstring_owned(s: &str) -> ObjcId {
        unsafe {
            let ns_string: ObjcId = msg_send![class!(NSString), alloc];
            if ns_string.is_null() {
                return nil;
            }
            msg_send![
                ns_string,
                initWithBytes: s.as_ptr() as *const c_void
                length: s.len() as u64
                encoding: UTF8_ENCODING
            ]
        }
    }

    fn ns_error_to_string(error: ObjcId) -> String {
        if error.is_null() {
            return "unknown Metal error".to_string();
        }
        unsafe {
            let desc: ObjcId = msg_send![error, localizedDescription];
            nsstring_to_string(desc)
        }
    }

    fn device_supports_family(device: ObjcId, family: u64) -> bool {
        unsafe { msg_send![device, supportsFamily: family] }
    }

    fn metal_compile_feature_macros(device: ObjcId) -> MetalDeviceFeatures {
        MetalDeviceFeatures {
            has_bfloat: device_supports_family(device, MTL_GPU_FAMILY_METAL3)
                || device_supports_family(device, MTL_GPU_FAMILY_APPLE6),
            has_tensor: device_supports_family(device, MTL_GPU_FAMILY_METAL4),
            has_simdgroup_mm: device_supports_family(device, MTL_GPU_FAMILY_APPLE7),
        }
    }

    fn read_text_with_fallback(paths: &[&str], fallback: &str) -> String {
        for path in paths {
            if let Ok(text) = std::fs::read_to_string(path) {
                return text;
            }
        }
        fallback.to_string()
    }

    fn build_ggml_source() -> String {
        let mut src = read_text_with_fallback(
            &[
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/backend/metal/ggml/ggml-metal.metal"
                ),
                "libs/ggml/src/backend/metal/ggml/ggml-metal.metal",
            ],
            GGML_METAL_SOURCE_RAW,
        );
        let common_h = read_text_with_fallback(
            &[
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/backend/metal/ggml/ggml-common.h"
                ),
                "libs/ggml/src/backend/metal/ggml/ggml-common.h",
            ],
            GGML_COMMON_H,
        );
        let impl_h = read_text_with_fallback(
            &[
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/backend/metal/ggml/ggml-metal-impl.h"
                ),
                "libs/ggml/src/backend/metal/ggml/ggml-metal-impl.h",
            ],
            GGML_METAL_IMPL_H,
        );

        src = src.replace("__embed_ggml-common.h__", &common_h);
        src = src.replace("#include \"ggml-common.h\"", &common_h);
        src = src.replace("#include \"ggml-metal-impl.h\"", &impl_h);
        src
    }

    #[derive(Clone, Debug)]
    pub struct MetalBuffer {
        obj: StrongId,
        size_bytes: usize,
        storage: BufferStorageMode,
    }

    impl MetalBuffer {
        pub fn size_bytes(&self) -> usize {
            self.size_bytes
        }

        pub fn storage(&self) -> BufferStorageMode {
            self.storage
        }

        pub(crate) fn as_id(&self) -> ObjcId {
            self.obj.as_id()
        }
    }

    #[derive(Clone, Debug)]
    pub struct MetalPipeline {
        #[allow(dead_code)]
        obj: StrongId,
        pub smem_bytes: usize,
        pub nr0: i32,
        pub nr1: i32,
        pub nsg: i32,
        pub max_threads_per_threadgroup: u64,
    }

    impl MetalPipeline {
        #[allow(dead_code)]
        pub(crate) fn as_id(&self) -> ObjcId {
            self.obj.as_id()
        }
    }

    struct MetalContext {
        device: StrongId,
        command_queue: StrongId,
        library: StrongId,
        pipeline_cache: HashMap<String, MetalPipeline>,
        last_command_buffer: Option<StrongId>,
    }

    pub struct MetalRuntime {
        ctx: RefCell<MetalContext>,
        info: BackendInfo,
        features: MetalDeviceFeatures,
    }

    impl MetalRuntime {
        pub fn is_available() -> bool {
            MetalContext::create_device().is_some()
        }

        pub fn new() -> MetalResult<Self> {
            let _pool = AutoreleasePool::new();
            let device = MetalContext::create_device()
                .ok_or_else(|| "unable to create Metal device".to_string())?;

            let name_obj: ObjcId = unsafe { msg_send![device.as_id(), name] };
            let name = nsstring_to_string(name_obj);
            let max_buffer_size: u64 = unsafe { msg_send![device.as_id(), maxBufferLength] };
            let features = metal_compile_feature_macros(device.as_id());

            let command_queue_obj: ObjcId = unsafe { msg_send![device.as_id(), newCommandQueue] };
            let command_queue = unsafe { StrongId::from_owned(command_queue_obj) }
                .ok_or_else(|| "newCommandQueue returned nil".to_string())?;

            let library = match MetalContext::load_library_from_metallib(device.as_id()) {
                Ok(Some(lib)) => lib,
                Ok(None) => {
                    let source = build_ggml_source();
                    MetalContext::compile_library(device.as_id(), &source)?
                }
                Err(err) => {
                    eprintln!(
                        "[ggml][metal] precompiled metallib load failed, compiling source: {}",
                        err
                    );
                    let source = build_ggml_source();
                    MetalContext::compile_library(device.as_id(), &source)?
                }
            };

            let info = BackendInfo {
                kind: BackendKind::Metal,
                name: name.clone(),
                description: format!("Apple Metal device '{}'", name),
                capabilities: BackendCapabilities {
                    bf16: features.has_bfloat,
                    tensor_cores: features.has_tensor,
                    max_buffer_size: usize::try_from(max_buffer_size).ok(),
                    max_threadgroup_memory: None,
                    subgroup_width: None,
                    asynchronous: false,
                    host_buffer: false,
                    buffer_from_host_ptr: false,
                    events: false,
                },
            };

            Ok(Self {
                ctx: RefCell::new(MetalContext {
                    device,
                    command_queue,
                    library,
                    pipeline_cache: HashMap::new(),
                    last_command_buffer: None,
                }),
                info,
                features,
            })
        }

        pub fn backend_info(&self) -> &BackendInfo {
            &self.info
        }

        pub fn features(&self) -> MetalDeviceFeatures {
            self.features
        }

        pub fn create_buffer_with_bytes(
            &self,
            bytes: &[u8],
            storage: BufferStorageMode,
        ) -> MetalResult<MetalBuffer> {
            if bytes.is_empty() {
                return self.create_buffer(0, storage);
            }
            let ctx = self.ctx.borrow();
            match storage {
                BufferStorageMode::Shared => {
                    let obj = ctx.new_buffer_with_bytes(bytes)?;
                    Ok(MetalBuffer {
                        obj,
                        size_bytes: bytes.len(),
                        storage,
                    })
                }
                BufferStorageMode::Private => {
                    let dst = ctx.new_buffer_with_length_private(bytes.len().max(1))?;
                    let staging = ctx.new_buffer_with_bytes(bytes)?;
                    ctx.copy_between_buffers(staging.as_id(), dst.as_id(), bytes.len())?;
                    Ok(MetalBuffer {
                        obj: dst,
                        size_bytes: bytes.len(),
                        storage,
                    })
                }
            }
        }

        pub fn create_buffer(
            &self,
            size_bytes: usize,
            storage: BufferStorageMode,
        ) -> MetalResult<MetalBuffer> {
            let ctx = self.ctx.borrow();
            let size_bytes = size_bytes.max(1);
            let obj = match storage {
                BufferStorageMode::Shared => ctx.new_buffer_with_length(size_bytes)?,
                BufferStorageMode::Private => ctx.new_buffer_with_length_private(size_bytes)?,
            };
            Ok(MetalBuffer {
                obj,
                size_bytes,
                storage,
            })
        }

        pub fn get_or_compile_pipeline(
            &self,
            desc: &MetalPipelineDescriptor,
        ) -> MetalResult<MetalPipeline> {
            let mut ctx = self.ctx.borrow_mut();
            if let Some(pipeline) = ctx.pipeline_cache.get(&desc.cache_name) {
                return Ok(pipeline.clone());
            }

            let obj = ctx.compile_pipeline(&desc.base_name, &desc.constants)?;
            let max_threads_per_threadgroup = MetalContext::pipeline_max_threads(obj.as_id());
            let pipeline = MetalPipeline {
                obj,
                smem_bytes: desc.smem_bytes,
                nr0: desc.nr0,
                nr1: desc.nr1,
                nsg: desc.nsg,
                max_threads_per_threadgroup,
            };
            ctx.pipeline_cache
                .insert(desc.cache_name.clone(), pipeline.clone());
            Ok(pipeline)
        }

        pub fn read_buffer(&self, buffer: &MetalBuffer, len_bytes: usize) -> MetalResult<Vec<u8>> {
            let ctx = self.ctx.borrow();
            if len_bytes > buffer.size_bytes {
                return Err(format!(
                    "requested read of {} bytes exceeds buffer size {}",
                    len_bytes, buffer.size_bytes
                ));
            }
            ctx.read_buffer_bytes(buffer.as_id(), len_bytes)
        }

        pub fn read_buffer_range(
            &self,
            buffer: &MetalBuffer,
            offset_bytes: usize,
            len_bytes: usize,
        ) -> MetalResult<Vec<u8>> {
            let ctx = self.ctx.borrow();
            if offset_bytes > buffer.size_bytes
                || len_bytes > buffer.size_bytes.saturating_sub(offset_bytes)
            {
                return Err(format!(
                    "requested read of {} bytes at offset {} exceeds buffer size {}",
                    len_bytes, offset_bytes, buffer.size_bytes
                ));
            }
            ctx.read_buffer_bytes_range(buffer.as_id(), offset_bytes, len_bytes)
        }

        pub fn write_buffer(
            &self,
            buffer: &MetalBuffer,
            offset_bytes: usize,
            bytes: &[u8],
        ) -> MetalResult<()> {
            let ctx = self.ctx.borrow();
            if offset_bytes > buffer.size_bytes
                || bytes.len() > buffer.size_bytes.saturating_sub(offset_bytes)
            {
                return Err(format!(
                    "requested write of {} bytes at offset {} exceeds buffer size {}",
                    bytes.len(),
                    offset_bytes,
                    buffer.size_bytes
                ));
            }
            ctx.write_buffer_bytes(buffer.as_id(), offset_bytes, bytes)
        }

        pub fn dispatch_compute(
            &self,
            pipeline: &MetalPipeline,
            args_bytes: &[u8],
            buffers: &[MetalBufferBindingRef<'_>],
            threadgroup_memory_lengths: &[(u64, usize)],
            threadgroups: MetalSize,
            threads_per_threadgroup: MetalSize,
        ) -> MetalResult<()> {
            self.ctx.borrow_mut().dispatch_compute(
                pipeline,
                args_bytes,
                buffers,
                threadgroup_memory_lengths,
                threadgroups,
                threads_per_threadgroup,
            )
        }

        pub fn wait_idle(&self) -> MetalResult<()> {
            self.ctx.borrow().wait_queue_idle()
        }
    }

    impl MetalContext {
        fn create_device() -> Option<StrongId> {
            unsafe {
                let dev = MTLCreateSystemDefaultDevice();
                if let Some(dev) = StrongId::from_owned(dev) {
                    return Some(dev);
                }

                let all = MTLCopyAllDevices();
                if all.is_null() {
                    return None;
                }

                let count: u64 = msg_send![all, count];
                let first: ObjcId = if count > 0 {
                    msg_send![all, objectAtIndex: 0u64]
                } else {
                    nil
                };
                let _: () = msg_send![all, release];

                StrongId::from_unowned(first)
            }
        }

        fn load_library_from_metallib(device: ObjcId) -> MetalResult<Option<StrongId>> {
            if GGML_METALLIB_BYTES.is_empty() {
                return Ok(None);
            }

            let _pool = AutoreleasePool::new();
            let data_obj: ObjcId = unsafe {
                msg_send![
                    class!(NSData),
                    dataWithBytes: GGML_METALLIB_BYTES.as_ptr() as *const c_void
                    length: GGML_METALLIB_BYTES.len() as u64
                ]
            };
            if data_obj.is_null() {
                return Err("NSData::dataWithBytes returned nil".to_string());
            }

            let mut error: ObjcId = nil;
            let library_obj: ObjcId =
                unsafe { msg_send![device, newLibraryWithData: data_obj error: &mut error] };
            if library_obj.is_null() {
                return Err(format!(
                    "newLibraryWithData failed: {}",
                    ns_error_to_string(error)
                ));
            }

            let library = unsafe { StrongId::from_owned(library_obj) }
                .ok_or_else(|| "newLibraryWithData returned nil".to_string())?;
            Ok(Some(library))
        }

        fn compile_library(device: ObjcId, source: &str) -> MetalResult<StrongId> {
            let _pool = AutoreleasePool::new();

            let options_obj: ObjcId = unsafe { msg_send![class!(MTLCompileOptions), new] };
            let options = unsafe { StrongId::from_owned(options_obj) }
                .ok_or_else(|| "MTLCompileOptions::new returned nil".to_string())?;
            unsafe {
                let _: () = msg_send![options.as_id(), setFastMathEnabled: YES];
            }

            let features = metal_compile_feature_macros(device);
            if features.has_bfloat || features.has_tensor {
                let prep_obj: ObjcId =
                    unsafe { msg_send![class!(NSMutableDictionary), dictionary] };
                if !prep_obj.is_null() {
                    if features.has_bfloat {
                        let key = unsafe {
                            StrongId::from_owned(str_to_nsstring_owned("GGML_METAL_HAS_BF16"))
                        }
                        .ok_or_else(|| "failed to build metal macro key".to_string())?;
                        let val = unsafe { StrongId::from_owned(str_to_nsstring_owned("1")) }
                            .ok_or_else(|| "failed to build metal macro value".to_string())?;
                        unsafe {
                            let _: () =
                                msg_send![prep_obj, setObject: val.as_id() forKey: key.as_id()];
                        }
                    }
                    if features.has_tensor {
                        let key = unsafe {
                            StrongId::from_owned(str_to_nsstring_owned("GGML_METAL_HAS_TENSOR"))
                        }
                        .ok_or_else(|| "failed to build metal macro key".to_string())?;
                        let val = unsafe { StrongId::from_owned(str_to_nsstring_owned("1")) }
                            .ok_or_else(|| "failed to build metal macro value".to_string())?;
                        unsafe {
                            let _: () =
                                msg_send![prep_obj, setObject: val.as_id() forKey: key.as_id()];
                        }
                    }
                    unsafe {
                        let _: () = msg_send![options.as_id(), setPreprocessorMacros: prep_obj];
                    }
                }
            }

            let source_obj = unsafe { StrongId::from_owned(str_to_nsstring_owned(source)) }
                .ok_or_else(|| "failed to create NSString for Metal source".to_string())?;

            let mut error: ObjcId = nil;
            let library_obj: ObjcId = unsafe {
                msg_send![
                    device,
                    newLibraryWithSource: source_obj.as_id()
                    options: options.as_id()
                    error: &mut error
                ]
            };

            unsafe { StrongId::from_owned(library_obj) }.ok_or_else(|| {
                format!("newLibraryWithSource failed: {}", ns_error_to_string(error))
            })
        }

        fn new_buffer_with_bytes(&self, bytes: &[u8]) -> MetalResult<StrongId> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithBytes: bytes.as_ptr() as *const c_void
                    length: bytes.len().max(1) as u64
                    options: MTL_RESOURCE_STORAGE_MODE_SHARED
                ]
            };
            unsafe { StrongId::from_owned(obj) }
                .ok_or_else(|| format!("newBufferWithBytes failed for {} bytes", bytes.len()))
        }

        fn new_buffer_with_length(&self, byte_len: usize) -> MetalResult<StrongId> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithLength: byte_len as u64
                    options: MTL_RESOURCE_STORAGE_MODE_SHARED
                ]
            };
            unsafe { StrongId::from_owned(obj) }
                .ok_or_else(|| format!("newBufferWithLength failed for {} bytes", byte_len))
        }

        fn new_buffer_with_length_private(&self, byte_len: usize) -> MetalResult<StrongId> {
            let obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newBufferWithLength: byte_len as u64
                    options: MTL_RESOURCE_OPTIONS_STORAGE_MODE_PRIVATE
                ]
            };
            unsafe { StrongId::from_owned(obj) }.ok_or_else(|| {
                format!("newBufferWithLength(private) failed for {} bytes", byte_len)
            })
        }

        fn buffer_length_bytes(&self, buffer: ObjcId) -> MetalResult<usize> {
            let len_u64: u64 = unsafe { msg_send![buffer, length] };
            usize::try_from(len_u64).map_err(|_| format!("buffer length too large: {}", len_u64))
        }

        fn copy_between_buffers(
            &self,
            src_buffer: ObjcId,
            dst_buffer: ObjcId,
            len_bytes: usize,
        ) -> MetalResult<()> {
            self.copy_between_buffers_ranges(src_buffer, 0, dst_buffer, 0, len_bytes)
        }

        fn copy_between_buffers_ranges(
            &self,
            src_buffer: ObjcId,
            src_offset: usize,
            dst_buffer: ObjcId,
            dst_offset: usize,
            len_bytes: usize,
        ) -> MetalResult<()> {
            let len_bytes = len_bytes.max(1);
            let src_len = self.buffer_length_bytes(src_buffer)?;
            let dst_len = self.buffer_length_bytes(dst_buffer)?;
            if src_offset > src_len
                || dst_offset > dst_len
                || len_bytes > src_len.saturating_sub(src_offset)
                || len_bytes > dst_len.saturating_sub(dst_offset)
            {
                return Err(format!(
                    "buffer copy exceeds bounds: src_offset={}, dst_offset={}, len={}, src_len={}, dst_len={}",
                    src_offset, dst_offset, len_bytes, src_len, dst_len
                ));
            }

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;

            let blit_encoder_obj: ObjcId =
                unsafe { msg_send![command_buffer.as_id(), blitCommandEncoder] };
            let blit_encoder = unsafe { StrongId::from_unowned(blit_encoder_obj) }
                .ok_or_else(|| "blitCommandEncoder returned nil".to_string())?;

            unsafe {
                let _: () = msg_send![
                    blit_encoder.as_id(),
                    copyFromBuffer: src_buffer
                    sourceOffset: src_offset as u64
                    toBuffer: dst_buffer
                    destinationOffset: dst_offset as u64
                    size: len_bytes as u64
                ];
                let _: () = msg_send![blit_encoder.as_id(), endEncoding];
                let _: () = msg_send![command_buffer.as_id(), commit];
                let _: () = msg_send![command_buffer.as_id(), waitUntilCompleted];
            }

            let status: u64 = unsafe { msg_send![command_buffer.as_id(), status] };
            if status == 5 {
                let error: ObjcId = unsafe { msg_send![command_buffer.as_id(), error] };
                return Err(format!(
                    "Metal command buffer error (buffer copy): {}",
                    ns_error_to_string(error)
                ));
            }

            Ok(())
        }

        fn copy_buffer_to_shared_staging(
            &self,
            src_buffer: ObjcId,
            len_bytes: usize,
        ) -> MetalResult<StrongId> {
            let dst = self.new_buffer_with_length(len_bytes)?;
            self.copy_between_buffers(src_buffer, dst.as_id(), len_bytes)?;
            Ok(dst)
        }

        fn copy_buffer_range_to_shared_staging(
            &self,
            src_buffer: ObjcId,
            src_offset: usize,
            len_bytes: usize,
        ) -> MetalResult<StrongId> {
            let dst = self.new_buffer_with_length(len_bytes.max(1))?;
            self.copy_between_buffers_ranges(src_buffer, src_offset, dst.as_id(), 0, len_bytes)?;
            Ok(dst)
        }

        fn read_buffer_bytes(&self, buffer: ObjcId, len_bytes: usize) -> MetalResult<Vec<u8>> {
            self.read_buffer_bytes_range(buffer, 0, len_bytes)
        }

        fn read_buffer_bytes_range(
            &self,
            buffer: ObjcId,
            offset_bytes: usize,
            len_bytes: usize,
        ) -> MetalResult<Vec<u8>> {
            self.wait_queue_idle()?;
            let cap = self.buffer_length_bytes(buffer)?;
            if offset_bytes > cap || len_bytes > cap.saturating_sub(offset_bytes) {
                return Err(format!(
                    "requested read of {} bytes at offset {} exceeds buffer size {}",
                    len_bytes, offset_bytes, cap
                ));
            }

            let (readable, readable_offset) = {
                let storage_mode: u64 = unsafe { msg_send![buffer, storageMode] };
                if storage_mode == MTL_STORAGE_MODE_PRIVATE {
                    (
                        self.copy_buffer_range_to_shared_staging(buffer, offset_bytes, len_bytes)?,
                        0usize,
                    )
                } else {
                    (
                        unsafe { StrongId::from_unowned(buffer) }
                            .ok_or_else(|| "buffer handle became invalid".to_string())?,
                        offset_bytes,
                    )
                }
            };

            let ptr: *const u8 = unsafe { msg_send![readable.as_id(), contents] };
            if ptr.is_null() {
                return Err("buffer contents returned null".to_string());
            }

            let mut out = vec![0u8; len_bytes];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ptr.add(readable_offset),
                    out.as_mut_ptr(),
                    len_bytes,
                );
            }
            Ok(out)
        }

        fn write_buffer_bytes(
            &self,
            buffer: ObjcId,
            offset_bytes: usize,
            bytes: &[u8],
        ) -> MetalResult<()> {
            let cap = self.buffer_length_bytes(buffer)?;
            if offset_bytes > cap || bytes.len() > cap.saturating_sub(offset_bytes) {
                return Err(format!(
                    "requested write of {} bytes at offset {} exceeds buffer size {}",
                    bytes.len(),
                    offset_bytes,
                    cap
                ));
            }

            let storage_mode: u64 = unsafe { msg_send![buffer, storageMode] };
            if storage_mode == MTL_STORAGE_MODE_PRIVATE {
                let staging = self.new_buffer_with_bytes(bytes)?;
                self.copy_between_buffers_ranges(
                    staging.as_id(),
                    0,
                    buffer,
                    offset_bytes,
                    bytes.len(),
                )?;
                return Ok(());
            }

            let ptr: *mut u8 = unsafe { msg_send![buffer, contents] };
            if ptr.is_null() {
                return Err("buffer contents returned null".to_string());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.add(offset_bytes), bytes.len());
            }
            Ok(())
        }

        fn compile_pipeline(
            &self,
            base_name: &str,
            constants: &[FunctionConstant],
        ) -> MetalResult<StrongId> {
            let _pool = AutoreleasePool::new();
            let base_obj = unsafe { StrongId::from_owned(str_to_nsstring_owned(base_name)) }
                .ok_or_else(|| format!("failed to create NSString for '{}'", base_name))?;

            let mut error: ObjcId = nil;
            let func_obj: ObjcId = if constants.is_empty() {
                unsafe { msg_send![self.library.as_id(), newFunctionWithName: base_obj.as_id()] }
            } else {
                let cv_obj: ObjcId = unsafe { msg_send![class!(MTLFunctionConstantValues), new] };
                let cv = unsafe { StrongId::from_owned(cv_obj) }
                    .ok_or_else(|| "MTLFunctionConstantValues::new returned nil".to_string())?;

                for c in constants {
                    unsafe {
                        match c.value {
                            FunctionConstantValue::Int32(v) => {
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &v as *const i32 as *const c_void
                                    type: MTL_DATA_TYPE_INT
                                    atIndex: c.idx as u64
                                ];
                            }
                            FunctionConstantValue::Int16(v) => {
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &v as *const i16 as *const c_void
                                    type: MTL_DATA_TYPE_SHORT
                                    atIndex: c.idx as u64
                                ];
                            }
                            FunctionConstantValue::Bool(v) => {
                                let b: u8 = if v { 1 } else { 0 };
                                let _: () = msg_send![
                                    cv.as_id(),
                                    setConstantValue: &b as *const u8 as *const c_void
                                    type: MTL_DATA_TYPE_BOOL
                                    atIndex: c.idx as u64
                                ];
                            }
                        }
                    }
                }

                unsafe {
                    msg_send![
                        self.library.as_id(),
                        newFunctionWithName: base_obj.as_id()
                        constantValues: cv.as_id()
                        error: &mut error
                    ]
                }
            };

            let func = unsafe { StrongId::from_owned(func_obj) }.ok_or_else(|| {
                format!(
                    "newFunctionWithName('{}') failed: {}",
                    base_name,
                    ns_error_to_string(error)
                )
            })?;

            let mut error: ObjcId = nil;
            let pipeline_obj: ObjcId = unsafe {
                msg_send![
                    self.device.as_id(),
                    newComputePipelineStateWithFunction: func.as_id()
                    error: &mut error
                ]
            };

            unsafe { StrongId::from_owned(pipeline_obj) }.ok_or_else(|| {
                format!(
                    "newComputePipelineStateWithFunction('{}') failed: {}",
                    base_name,
                    ns_error_to_string(error)
                )
            })
        }

        fn pipeline_max_threads(pipeline: ObjcId) -> u64 {
            unsafe { msg_send![pipeline, maxTotalThreadsPerThreadgroup] }
        }

        fn dispatch_compute(
            &mut self,
            pipeline: &MetalPipeline,
            args_bytes: &[u8],
            buffers: &[MetalBufferBindingRef<'_>],
            threadgroup_memory_lengths: &[(u64, usize)],
            threadgroups: MetalSize,
            threads_per_threadgroup: MetalSize,
        ) -> MetalResult<()> {
            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;
            let encoder_obj: ObjcId =
                unsafe { msg_send![command_buffer.as_id(), computeCommandEncoder] };
            let encoder = unsafe { StrongId::from_unowned(encoder_obj) }
                .ok_or_else(|| "computeCommandEncoder returned nil".to_string())?;

            unsafe {
                let _: () = msg_send![encoder.as_id(), setComputePipelineState: pipeline.as_id()];
                if !args_bytes.is_empty() {
                    let _: () = msg_send![
                        encoder.as_id(),
                        setBytes: args_bytes.as_ptr() as *const c_void
                        length: args_bytes.len() as u64
                        atIndex: 0u64
                    ];
                }

                for binding in buffers {
                    let _: () = msg_send![
                        encoder.as_id(),
                        setBuffer: binding.buffer.as_id()
                        offset: binding.offset_bytes as u64
                        atIndex: binding.index
                    ];
                }

                for &(index, length) in threadgroup_memory_lengths {
                    let _: () = msg_send![
                        encoder.as_id(),
                        setThreadgroupMemoryLength: length as u64
                        atIndex: index
                    ];
                }

                let tgs = MTLSize {
                    width: threadgroups.width,
                    height: threadgroups.height,
                    depth: threadgroups.depth,
                };
                let tpg = MTLSize {
                    width: threads_per_threadgroup.width,
                    height: threads_per_threadgroup.height,
                    depth: threads_per_threadgroup.depth,
                };
                let _: () = msg_send![
                    encoder.as_id(),
                    dispatchThreadgroups: tgs
                    threadsPerThreadgroup: tpg
                ];
                let _: () = msg_send![encoder.as_id(), endEncoding];
                let _: () = msg_send![command_buffer.as_id(), commit];
            }

            self.last_command_buffer = Some(command_buffer);
            Ok(())
        }

        fn wait_queue_idle(&self) -> MetalResult<()> {
            if let Some(command_buffer) = self.last_command_buffer.as_ref() {
                unsafe {
                    let _: () = msg_send![command_buffer.as_id(), waitUntilCompleted];
                }
                let status: u64 = unsafe { msg_send![command_buffer.as_id(), status] };
                if status == 5 {
                    let error: ObjcId = unsafe { msg_send![command_buffer.as_id(), error] };
                    return Err(format!(
                        "Metal command buffer error (queue idle wait): {}",
                        ns_error_to_string(error)
                    ));
                }
                return Ok(());
            }

            let command_buffer_obj: ObjcId =
                unsafe { msg_send![self.command_queue.as_id(), commandBuffer] };
            let command_buffer = unsafe { StrongId::from_unowned(command_buffer_obj) }
                .ok_or_else(|| "commandBuffer returned nil".to_string())?;
            unsafe {
                let _: () = msg_send![command_buffer.as_id(), commit];
                let _: () = msg_send![command_buffer.as_id(), waitUntilCompleted];
            }
            let status: u64 = unsafe { msg_send![command_buffer.as_id(), status] };
            if status == 5 {
                let error: ObjcId = unsafe { msg_send![command_buffer.as_id(), error] };
                return Err(format!(
                    "Metal command buffer error (queue idle wait): {}",
                    ns_error_to_string(error)
                ));
            }
            Ok(())
        }
    }
}

pub use imp::*;
