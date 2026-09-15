use {
    crate::{
        cx::Cx,
        event::Event,
        thread::ToUIReceiver,
    },
    std::collections::HashMap,
};

/// Default maximum size of one stored value. Writes are whole-value for now;
/// streaming or append-style writes can be added without changing
/// [`StorageHandle`].
pub const DEFAULT_STORAGE_VALUE_CAP: usize = 64 * 1024 * 1024;
pub const MAX_STORAGE_NAMESPACE_BYTES: usize = 64;
pub const MAX_STORAGE_KEY_BYTES: usize = 1024;
pub const MAX_STORAGE_LIST_LIMIT: u32 = 1024;

/// Identifier returned immediately when an asynchronous storage operation is
/// submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageRequestId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Operation associated with a [`StorageResponse`].
pub enum StorageOp {
    Get,
    Set,
    Delete,
    List,
    GetRange,
    Stat,
    Estimate,
}

impl StorageOp {
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Get,
            1 => Self::Set,
            2 => Self::Delete,
            3 => Self::List,
            4 => Self::GetRange,
            5 => Self::Stat,
            6 => Self::Estimate,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// One bounded, sorted page returned by [`StorageHandle::list`].
pub struct StorageList {
    /// Sorted keys in this page.
    pub keys: Vec<String>,
    /// Pass this value as `after` to request the next page.
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Metadata returned by [`StorageHandle::stat`].
pub struct StorageStat {
    pub len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Best-effort storage usage and quota for the current origin or native
/// storage volume.
pub struct StorageEstimate {
    pub usage: u64,
    pub quota: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Successful payload of a storage response.
pub enum StorageResult {
    /// Result of `get` or `get_range`. `None` means the key was absent; an
    /// existing empty value is `Some(Vec::new())`.
    Value(Option<Vec<u8>>),
    List(StorageList),
    Stat(Option<StorageStat>),
    Estimate(StorageEstimate),
    /// Successful `set` or `delete`.
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Error reported asynchronously for a storage request.
pub enum StorageError {
    InvalidNamespace(String),
    InvalidKey(String),
    InvalidListLimit { limit: u32, max: u32 },
    ValueTooLarge { size: usize, max: usize },
    QuotaExceeded(String),
    Io(String),
    Unsupported(String),
    Backend(String),
    Protocol(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNamespace(message)
            | Self::InvalidKey(message)
            | Self::Io(message)
            | Self::Unsupported(message)
            | Self::Backend(message)
            | Self::Protocol(message) => f.write_str(message),
            Self::QuotaExceeded(message) => f.write_str(message),
            Self::InvalidListLimit { limit, max } => {
                write!(f, "storage list limit {limit} is outside 1..={max}")
            }
            Self::ValueTooLarge { size, max } => {
                write!(f, "storage value is {size} bytes; maximum is {max}")
            }
        }
    }
}

impl std::error::Error for StorageError {}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Completion record delivered in [`Event::Storage`].
pub struct StorageResponse {
    pub request_id: StorageRequestId,
    pub namespace: String,
    pub op: StorageOp,
    pub result: Result<StorageResult, StorageError>,
}

pub type StorageResponsesEvent = Vec<StorageResponse>;

/// A namespace-bound storage capability. Every operation performed through a
/// handle is scoped to this namespace; no un-namespaced entry point exists.
#[derive(Clone, Debug)]
pub struct StorageHandle {
    namespace: String,
    namespace_error: Option<StorageError>,
}

impl StorageHandle {
    /// Returns the validated namespace supplied to [`Cx::storage`].
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Reads a complete value. Missing and existing-empty values are distinct.
    pub fn get(&self, cx: &mut Cx, key: &str) -> StorageRequestId {
        self.submit(cx, StorageRequestKind::Get { key: key.into() })
    }

    /// Atomically replaces a complete value.
    pub fn set(
        &self,
        cx: &mut Cx,
        key: &str,
        bytes: impl Into<Vec<u8>>,
    ) -> StorageRequestId {
        self.submit(
            cx,
            StorageRequestKind::Set {
                key: key.into(),
                value: bytes.into(),
            },
        )
    }

    /// Deletes a value. Deleting a missing key succeeds.
    pub fn delete(&self, cx: &mut Cx, key: &str) -> StorageRequestId {
        self.submit(cx, StorageRequestKind::Delete { key: key.into() })
    }

    /// Lists a sorted, bounded page of keys beginning with `prefix`.
    /// Pass the preceding page's `next_cursor` as `after` to continue.
    pub fn list(
        &self,
        cx: &mut Cx,
        prefix: &str,
        after: Option<String>,
        limit: u32,
    ) -> StorageRequestId {
        self.submit(
            cx,
            StorageRequestKind::List {
                prefix: prefix.into(),
                after,
                limit,
            },
        )
    }

    /// Reads at most `len` bytes starting at `offset`, without returning the
    /// remainder of the value.
    pub fn get_range(
        &self,
        cx: &mut Cx,
        key: &str,
        offset: u64,
        len: u32,
    ) -> StorageRequestId {
        self.submit(
            cx,
            StorageRequestKind::GetRange {
                key: key.into(),
                offset,
                len,
            },
        )
    }

    /// Returns a value's byte length without reading its contents.
    pub fn stat(&self, cx: &mut Cx, key: &str) -> StorageRequestId {
        self.submit(cx, StorageRequestKind::Stat { key: key.into() })
    }

    /// Estimates total storage usage and quota without escaping this
    /// namespace-bound capability.
    pub fn estimate(&self, cx: &mut Cx) -> StorageRequestId {
        self.submit(cx, StorageRequestKind::Estimate)
    }

    fn submit(&self, cx: &mut Cx, kind: StorageRequestKind) -> StorageRequestId {
        let op = kind.op();
        let request_id = cx.storage_state.begin(self.namespace.clone(), op);
        let request = StorageRequest {
            request_id,
            namespace: self.namespace.clone(),
            kind,
        };

        let validation = self
            .namespace_error
            .clone()
            .map_or_else(|| request.validate(cx.storage_state.value_cap), Err);
        if let Err(error) = validation {
            #[cfg(target_arch = "wasm32")]
            cx.platform_ops
                .push_back(crate::cx_api::CxOsOp::StorageRequestError {
                    request_id,
                    op,
                    error,
                });
            #[cfg(not(target_arch = "wasm32"))]
            cx.storage_state
                .queue(StorageWorkerResponse::new(request_id, op, Err(error)));
            return request_id;
        }

        #[cfg(target_arch = "wasm32")]
        cx.platform_ops
            .push_back(crate::cx_api::CxOsOp::StorageRequest(request));

        #[cfg(all(
            not(target_arch = "wasm32"),
            any(
                target_os = "android",
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos",
                target_os = "windows"
            )
        ))]
        cx.start_native_storage_request(request);

        #[cfg(all(
            not(target_arch = "wasm32"),
            not(any(
                target_os = "android",
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos",
                target_os = "windows"
            ))
        ))]
        cx.storage_state.queue(StorageWorkerResponse::new(
            request_id,
            op,
            Err(StorageError::Unsupported(
                "storage is unsupported on this platform".into(),
            )),
        ));

        request_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StorageRequest {
    pub(crate) request_id: StorageRequestId,
    pub(crate) namespace: String,
    pub(crate) kind: StorageRequestKind,
}

impl StorageRequest {
    fn validate(&self, value_cap: usize) -> Result<(), StorageError> {
        match &self.kind {
            StorageRequestKind::Get { key }
            | StorageRequestKind::Delete { key }
            | StorageRequestKind::Stat { key } => validate_key(key),
            StorageRequestKind::Set { key, value } => {
                validate_key(key)?;
                if value.len() > value_cap {
                    return Err(StorageError::ValueTooLarge {
                        size: value.len(),
                        max: value_cap,
                    });
                }
                Ok(())
            }
            StorageRequestKind::List {
                prefix,
                after,
                limit,
            } => {
                validate_key(prefix)?;
                if let Some(after) = after {
                    validate_key(after)?;
                }
                if *limit == 0 || *limit > MAX_STORAGE_LIST_LIMIT {
                    return Err(StorageError::InvalidListLimit {
                        limit: *limit,
                        max: MAX_STORAGE_LIST_LIMIT,
                    });
                }
                Ok(())
            }
            StorageRequestKind::GetRange { key, len, .. } => {
                validate_key(key)?;
                if *len as usize > value_cap {
                    return Err(StorageError::ValueTooLarge {
                        size: *len as usize,
                        max: value_cap,
                    });
                }
                Ok(())
            }
            StorageRequestKind::Estimate => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StorageRequestKind {
    Get {
        key: String,
    },
    Set {
        key: String,
        value: Vec<u8>,
    },
    Delete {
        key: String,
    },
    List {
        prefix: String,
        after: Option<String>,
        limit: u32,
    },
    GetRange {
        key: String,
        offset: u64,
        len: u32,
    },
    Stat {
        key: String,
    },
    Estimate,
}

impl StorageRequestKind {
    pub(crate) fn op(&self) -> StorageOp {
        match self {
            Self::Get { .. } => StorageOp::Get,
            Self::Set { .. } => StorageOp::Set,
            Self::Delete { .. } => StorageOp::Delete,
            Self::List { .. } => StorageOp::List,
            Self::GetRange { .. } => StorageOp::GetRange,
            Self::Stat { .. } => StorageOp::Stat,
            Self::Estimate => StorageOp::Estimate,
        }
    }
}

#[derive(Debug)]
pub(crate) struct StorageWorkerResponse {
    request_id: StorageRequestId,
    op: StorageOp,
    result: Result<StorageResult, StorageError>,
}

impl StorageWorkerResponse {
    pub(crate) fn new(
        request_id: StorageRequestId,
        op: StorageOp,
        result: Result<StorageResult, StorageError>,
    ) -> Self {
        Self {
            request_id,
            op,
            result,
        }
    }
}

#[derive(Clone, Debug)]
struct PendingStorageRequest {
    namespace: String,
    op: StorageOp,
}

pub(crate) struct StorageState {
    next_request_id: u64,
    pending: HashMap<StorageRequestId, PendingStorageRequest>,
    responses: ToUIReceiver<StorageWorkerResponse>,
    value_cap: usize,
}

impl Default for StorageState {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            pending: HashMap::new(),
            responses: ToUIReceiver::default(),
            value_cap: DEFAULT_STORAGE_VALUE_CAP,
        }
    }
}

impl StorageState {
    fn begin(&mut self, namespace: String, op: StorageOp) -> StorageRequestId {
        let request_id = StorageRequestId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending
            .insert(request_id, PendingStorageRequest { namespace, op });
        request_id
    }

    fn finish(&mut self, response: StorageWorkerResponse) -> Option<StorageResponse> {
        let pending = self.pending.remove(&response.request_id)?;
        let result = if pending.op == response.op {
            response.result
        } else {
            Err(StorageError::Protocol(format!(
                "storage response operation mismatch: expected {:?}, received {:?}",
                pending.op, response.op
            )))
        };
        Some(StorageResponse {
            request_id: response.request_id,
            namespace: pending.namespace,
            op: pending.op,
            result,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn queue(&self, response: StorageWorkerResponse) {
        let _ = self.responses.sender().send(response);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn sender(&self) -> crate::thread::ToUISender<StorageWorkerResponse> {
        self.responses.sender()
    }
}

impl Cx {
    /// Creates a namespace-bound storage handle. Invalid namespaces still
    /// produce a handle so this infallible API remains convenient; every
    /// operation on that handle completes asynchronously with
    /// [`StorageError::InvalidNamespace`].
    pub fn storage(&self, namespace: &str) -> StorageHandle {
        StorageHandle {
            namespace: namespace.into(),
            namespace_error: validate_namespace(namespace).err(),
        }
    }

    /// Changes the whole-value write and ranged-read response cap for future
    /// requests. The default is [`DEFAULT_STORAGE_VALUE_CAP`].
    pub fn set_storage_value_cap(&mut self, max_bytes: usize) {
        self.storage_state.value_cap = max_bytes;
    }

    pub fn storage_value_cap(&self) -> usize {
        self.storage_state.value_cap
    }

    pub(crate) fn dispatch_storage_responses(&mut self) {
        let mut responses = Vec::new();
        while let Ok(response) = self.storage_state.responses.try_recv() {
            if let Some(response) = self.storage_state.finish(response) {
                responses.push(response);
            }
        }
        if !responses.is_empty() {
            self.call_event_handler(&Event::Storage(responses));
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn finish_web_storage_request(
        &mut self,
        request_id: StorageRequestId,
        op: StorageOp,
        result: Result<StorageResult, StorageError>,
    ) -> Option<StorageResponse> {
        self.storage_state
            .finish(StorageWorkerResponse::new(request_id, op, result))
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn finish_web_storage_protocol_error(
        &mut self,
        request_id: StorageRequestId,
        message: String,
    ) -> Option<StorageResponse> {
        let op = self.storage_state.pending.get(&request_id)?.op;
        self.storage_state.finish(StorageWorkerResponse::new(
            request_id,
            op,
            Err(StorageError::Protocol(message)),
        ))
    }
}

fn validate_namespace(namespace: &str) -> Result<(), StorageError> {
    if namespace.is_empty() {
        return Err(StorageError::InvalidNamespace(
            "storage namespace must not be empty".into(),
        ));
    }
    if namespace.len() > MAX_STORAGE_NAMESPACE_BYTES {
        return Err(StorageError::InvalidNamespace(format!(
            "storage namespace is {} bytes; maximum is {}",
            namespace.len(),
            MAX_STORAGE_NAMESPACE_BYTES
        )));
    }
    if namespace == "."
        || namespace == ".."
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(StorageError::InvalidNamespace(
            "storage namespace may contain only ASCII letters, digits, '.', '_' and '-'".into(),
        ));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), StorageError> {
    if key.len() > MAX_STORAGE_KEY_BYTES {
        return Err(StorageError::InvalidKey(format!(
            "storage key is {} bytes; maximum is {}",
            key.len(),
            MAX_STORAGE_KEY_BYTES
        )));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod native {
    use {
        super::*,
        std::{
            fs::{self, File, OpenOptions},
            io::{Read, Seek, SeekFrom, Write},
            path::{Path, PathBuf},
            sync::atomic::{AtomicU64, Ordering},
        },
    };

    const FILE_SUFFIX: &str = ".mpkv";
    const PATH_CHUNK_BYTES: usize = 120;
    static TEMP_FILE_ID: AtomicU64 = AtomicU64::new(1);

    pub(crate) fn execute(root: &Path, request: StorageRequest) -> StorageWorkerResponse {
        let op = request.kind.op();
        let result = execute_inner(root, &request.namespace, request.kind);
        StorageWorkerResponse::new(request.request_id, op, result)
    }

    fn execute_inner(
        root: &Path,
        namespace: &str,
        kind: StorageRequestKind,
    ) -> Result<StorageResult, StorageError> {
        let namespace_dir = root.join(namespace);
        match kind {
            StorageRequestKind::Get { key } => {
                let path = key_path(&namespace_dir, &key);
                match fs::read(path) {
                    Ok(value) => Ok(StorageResult::Value(Some(value))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(StorageResult::Value(None))
                    }
                    Err(error) => Err(io_error("read", error)),
                }
            }
            StorageRequestKind::Set { key, value } => {
                atomic_write(&key_path(&namespace_dir, &key), &value)?;
                Ok(StorageResult::Unit)
            }
            StorageRequestKind::Delete { key } => {
                match fs::remove_file(key_path(&namespace_dir, &key)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(io_error("delete", error)),
                }
                Ok(StorageResult::Unit)
            }
            StorageRequestKind::List {
                prefix,
                after,
                limit,
            } => {
                let mut keys = Vec::new();
                collect_keys(&namespace_dir, &namespace_dir, &mut keys)?;
                keys.sort_unstable();
                keys.retain(|key| {
                    key.starts_with(&prefix)
                        && after.as_ref().map_or(true, |after| key.as_str() > after.as_str())
                });
                let has_more = keys.len() > limit as usize;
                keys.truncate(limit as usize);
                let next_cursor = has_more.then(|| keys.last().cloned()).flatten();
                Ok(StorageResult::List(StorageList { keys, next_cursor }))
            }
            StorageRequestKind::GetRange {
                key,
                offset,
                len,
            } => {
                let path = key_path(&namespace_dir, &key);
                let mut file = match File::open(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(StorageResult::Value(None));
                    }
                    Err(error) => return Err(io_error("open range", error)),
                };
                let file_len = file.metadata().map_err(|error| io_error("stat", error))?.len();
                if offset >= file_len || len == 0 {
                    return Ok(StorageResult::Value(Some(Vec::new())));
                }
                file.seek(SeekFrom::Start(offset))
                    .map_err(|error| io_error("seek", error))?;
                let read_len = (file_len - offset).min(len as u64) as usize;
                let mut value = vec![0; read_len];
                file.read_exact(&mut value)
                    .map_err(|error| io_error("read range", error))?;
                Ok(StorageResult::Value(Some(value)))
            }
            StorageRequestKind::Stat { key } => {
                match fs::metadata(key_path(&namespace_dir, &key)) {
                    Ok(metadata) => Ok(StorageResult::Stat(Some(StorageStat {
                        len: metadata.len(),
                    }))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(StorageResult::Stat(None))
                    }
                    Err(error) => Err(io_error("stat", error)),
                }
            }
            StorageRequestKind::Estimate => storage_estimate(root),
        }
    }

    fn storage_estimate(root: &Path) -> Result<StorageResult, StorageError> {
        let usage = tree_usage(root)?;
        let probe = root
            .ancestors()
            .find(|path| path.exists())
            .ok_or_else(|| StorageError::Io("storage estimate has no existing ancestor".into()))?;
        let available = volume_available_bytes(probe)?;
        Ok(StorageResult::Estimate(StorageEstimate {
            usage,
            quota: usage.saturating_add(available),
        }))
    }

    fn tree_usage(path: &Path) -> Result<u64, StorageError> {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(io_error("estimate usage", error)),
        };
        let mut total = 0u64;
        for entry in entries {
            let entry = entry.map_err(|error| io_error("estimate usage entry", error))?;
            let metadata = entry
                .metadata()
                .map_err(|error| io_error("estimate usage metadata", error))?;
            total = total.saturating_add(if metadata.is_dir() {
                tree_usage(&entry.path())?
            } else {
                metadata.len()
            });
        }
        Ok(total)
    }

    #[cfg(all(
        unix,
        target_pointer_width = "64",
        not(any(target_os = "macos", target_os = "ios", target_os = "tvos"))
    ))]
    fn volume_available_bytes(path: &Path) -> Result<u64, StorageError> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        #[repr(C)]
        struct StatVfs {
            block_size: usize,
            fragment_size: usize,
            blocks: u64,
            blocks_free: u64,
            blocks_available: u64,
            files: u64,
            files_free: u64,
            files_available: u64,
            filesystem_id: usize,
            flags: usize,
            name_max: usize,
            spare: [u64; 32],
        }
        unsafe extern "C" {
            fn statvfs(path: *const std::ffi::c_char, out: *mut StatVfs) -> std::ffi::c_int;
        }

        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| StorageError::Io("storage estimate path contains NUL".into()))?;
        let mut stat: StatVfs = unsafe { std::mem::zeroed() };
        if unsafe { statvfs(path.as_ptr(), &mut stat) } != 0 {
            return Err(io_error("estimate free space", std::io::Error::last_os_error()));
        }
        Ok(stat
            .fragment_size
            .max(stat.block_size) as u64
            * stat.blocks_available)
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    fn volume_available_bytes(path: &Path) -> Result<u64, StorageError> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        #[repr(C)]
        struct StatFs {
            block_size: u32,
            io_size: i32,
            blocks: u64,
            blocks_free: u64,
            blocks_available: u64,
            files: u64,
            files_free: u64,
            filesystem_id: [i32; 2],
            owner: u32,
            filesystem_type: u32,
            flags: u32,
            filesystem_subtype: u32,
            filesystem_type_name: [u8; 16],
            mount_on_name: [u8; 1024],
            mount_from_name: [u8; 1024],
            flags_ext: u32,
            reserved: [u32; 7],
        }
        unsafe extern "C" {
            fn statfs(path: *const std::ffi::c_char, out: *mut StatFs) -> std::ffi::c_int;
        }

        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| StorageError::Io("storage estimate path contains NUL".into()))?;
        let mut stat: StatFs = unsafe { std::mem::zeroed() };
        if unsafe { statfs(path.as_ptr(), &mut stat) } != 0 {
            return Err(io_error(
                "estimate free space",
                std::io::Error::last_os_error(),
            ));
        }
        Ok((stat.block_size as u64).saturating_mul(stat.blocks_available))
    }

    #[cfg(windows)]
    fn volume_available_bytes(path: &Path) -> Result<u64, StorageError> {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                directory: *const u16,
                available: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available = 0;
        let mut total = 0;
        let mut free = 0;
        if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
            return Err(io_error("estimate free space", std::io::Error::last_os_error()));
        }
        Ok(available)
    }

    #[cfg(not(any(windows, all(unix, target_pointer_width = "64"))))]
    fn volume_available_bytes(_path: &Path) -> Result<u64, StorageError> {
        Err(StorageError::Unsupported(
            "native storage quota estimate is unsupported on this target".into(),
        ))
    }

    fn atomic_write(path: &Path, value: &[u8]) -> Result<(), StorageError> {
        let parent = path.parent().ok_or_else(|| {
            StorageError::Io("storage key path has no parent directory".into())
        })?;
        fs::create_dir_all(parent).map_err(|error| io_error("create directory", error))?;

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("value.mpkv");
        let temp_path = parent.join(format!(
            ".{file_name}.tmp.{}.{}",
            std::process::id(),
            TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .map_err(|error| io_error("create temporary value", error))?;
            file.write_all(value)
                .map_err(|error| io_error("write temporary value", error))?;
            file.sync_all()
                .map_err(|error| io_error("sync temporary value", error))?;
            replace_file(&temp_path, path).map_err(|error| io_error("commit value", error))?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    #[cfg(not(target_os = "windows"))]
    fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }

    #[cfg(target_os = "windows")]
    fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }

        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn collect_keys(
        namespace_dir: &Path,
        directory: &Path,
        keys: &mut Vec<String>,
    ) -> Result<(), StorageError> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error("list", error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error("list entry", error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| io_error("list entry type", error))?;
            if file_type.is_dir() {
                collect_keys(namespace_dir, &entry.path(), keys)?;
            } else if file_type.is_file() {
                if let Some(key) = decode_key_path(namespace_dir, &entry.path()) {
                    keys.push(key);
                }
            }
        }
        Ok(())
    }

    fn key_path(namespace_dir: &Path, key: &str) -> PathBuf {
        let encoded = if key.is_empty() {
            "~".to_string()
        } else {
            hex_encode(key.as_bytes())
        };
        let mut path = namespace_dir.to_path_buf();
        let chunks: Vec<&str> = encoded
            .as_bytes()
            .chunks(PATH_CHUNK_BYTES)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect();
        for chunk in &chunks[..chunks.len().saturating_sub(1)] {
            path.push(chunk);
        }
        path.push(format!("{}{FILE_SUFFIX}", chunks.last().unwrap()));
        path
    }

    fn decode_key_path(namespace_dir: &Path, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(namespace_dir).ok()?;
        let mut encoded = String::new();
        let component_count = relative.components().count();
        for (index, component) in relative.components().enumerate() {
            let component = component.as_os_str().to_str()?;
            if index + 1 == component_count {
                encoded.push_str(component.strip_suffix(FILE_SUFFIX)?);
            } else {
                encoded.push_str(component);
            }
        }
        if encoded == "~" {
            return Some(String::new());
        }
        String::from_utf8(hex_decode(&encoded)?).ok()
    }

    fn hex_encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0xf) as usize] as char);
        }
        encoded
    }

    fn hex_decode(encoded: &str) -> Option<Vec<u8>> {
        if encoded.len() % 2 != 0 {
            return None;
        }
        encoded
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| Some((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?))
            .collect()
    }

    fn hex_digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }

    fn io_error(action: &str, error: std::io::Error) -> StorageError {
        StorageError::Io(format!("storage {action} failed: {error}"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{SystemTime, UNIX_EPOCH};

        struct TempDir(PathBuf);

        impl TempDir {
            fn new() -> Self {
                let unique = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!(
                    "makepad-platform-storage-test-{}-{unique}",
                    std::process::id()
                ));
                fs::create_dir(&path).unwrap();
                Self(path)
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        fn request(namespace: &str, id: u64, kind: StorageRequestKind) -> StorageRequest {
            StorageRequest {
                request_id: StorageRequestId(id),
                namespace: namespace.into(),
                kind,
            }
        }

        fn run(root: &Path, namespace: &str, id: u64, kind: StorageRequestKind) -> StorageResult {
            execute(root, request(namespace, id, kind)).result.unwrap()
        }

        #[test]
        fn native_round_trip_ranges_pages_namespaces_and_encoding() {
            let temp = TempDir::new();
            let encoded_key = "folder/../snow-☃";
            run(
                &temp.0,
                "app.one",
                1,
                StorageRequestKind::Set {
                    key: encoded_key.into(),
                    value: b"abcdef".to_vec(),
                },
            );
            run(
                &temp.0,
                "app.one",
                2,
                StorageRequestKind::Set {
                    key: "folder/z".into(),
                    value: Vec::new(),
                },
            );
            run(
                &temp.0,
                "app.two",
                3,
                StorageRequestKind::Set {
                    key: encoded_key.into(),
                    value: b"other".to_vec(),
                },
            );

            assert_eq!(
                run(
                    &temp.0,
                    "app.one",
                    4,
                    StorageRequestKind::Get {
                        key: encoded_key.into()
                    }
                ),
                StorageResult::Value(Some(b"abcdef".to_vec()))
            );
            run(
                &temp.0,
                "app.one",
                12,
                StorageRequestKind::Set {
                    key: encoded_key.into(),
                    value: b"updated".to_vec(),
                },
            );
            assert_eq!(
                run(
                    &temp.0,
                    "app.one",
                    13,
                    StorageRequestKind::Get {
                        key: encoded_key.into()
                    }
                ),
                StorageResult::Value(Some(b"updated".to_vec()))
            );
            run(
                &temp.0,
                "app.one",
                14,
                StorageRequestKind::Set {
                    key: encoded_key.into(),
                    value: b"abcdef".to_vec(),
                },
            );
            assert_eq!(
                run(
                    &temp.0,
                    "app.two",
                    5,
                    StorageRequestKind::Get {
                        key: encoded_key.into()
                    }
                ),
                StorageResult::Value(Some(b"other".to_vec()))
            );
            assert_eq!(
                run(
                    &temp.0,
                    "app.one",
                    6,
                    StorageRequestKind::GetRange {
                        key: encoded_key.into(),
                        offset: 2,
                        len: 3,
                    }
                ),
                StorageResult::Value(Some(b"cde".to_vec()))
            );
            assert_eq!(
                run(
                    &temp.0,
                    "app.one",
                    7,
                    StorageRequestKind::Stat {
                        key: encoded_key.into()
                    }
                ),
                StorageResult::Stat(Some(StorageStat { len: 6 }))
            );

            let first = run(
                &temp.0,
                "app.one",
                8,
                StorageRequestKind::List {
                    prefix: "folder/".into(),
                    after: None,
                    limit: 1,
                },
            );
            let StorageResult::List(first) = first else {
                panic!()
            };
            assert_eq!(first.keys, vec![encoded_key]);
            assert_eq!(first.next_cursor.as_deref(), Some(encoded_key));
            assert_eq!(
                run(
                    &temp.0,
                    "app.one",
                    9,
                    StorageRequestKind::List {
                        prefix: "folder/".into(),
                        after: first.next_cursor,
                        limit: 1,
                    }
                ),
                StorageResult::List(StorageList {
                    keys: vec!["folder/z".into()],
                    next_cursor: None,
                })
            );

            let stored_path = key_path(&temp.0.join("app.one"), encoded_key);
            assert!(stored_path.is_file());
            assert!(!temp.0.join("app.one/folder").exists());

            run(
                &temp.0,
                "app.one",
                10,
                StorageRequestKind::Delete {
                    key: encoded_key.into(),
                },
            );
            assert_eq!(
                run(
                    &temp.0,
                    "app.one",
                    11,
                    StorageRequestKind::Get {
                        key: encoded_key.into()
                    }
                ),
                StorageResult::Value(None)
            );
        }

        #[test]
        fn value_cap_and_crash_temp_file() {
            let request = request(
                "app",
                1,
                StorageRequestKind::Set {
                    key: "large".into(),
                    value: vec![0; 5],
                },
            );
            assert_eq!(
                request.validate(4),
                Err(StorageError::ValueTooLarge { size: 5, max: 4 })
            );

            let temp = TempDir::new();
            let namespace_dir = temp.0.join("app");
            run(
                &temp.0,
                "app",
                2,
                StorageRequestKind::Set {
                    key: "stable".into(),
                    value: b"committed".to_vec(),
                },
            );
            let stable_path = key_path(&namespace_dir, "stable");
            let stable_name = stable_path.file_name().unwrap().to_str().unwrap();
            fs::write(
                stable_path
                    .parent()
                    .unwrap()
                    .join(format!(".{stable_name}.tmp.crash")),
                b"partial",
            )
            .unwrap();
            assert_eq!(
                run(
                    &temp.0,
                    "app",
                    3,
                    StorageRequestKind::Get {
                        key: "stable".into()
                    }
                ),
                StorageResult::Value(Some(b"committed".to_vec()))
            );
            assert_eq!(
                run(
                    &temp.0,
                    "app",
                    4,
                    StorageRequestKind::List {
                        prefix: String::new(),
                        after: None,
                        limit: 10,
                    }
                ),
                StorageResult::List(StorageList {
                    keys: vec!["stable".into()],
                    next_cursor: None,
                })
            );
        }

        #[test]
        fn native_estimate_reports_usage_and_available_capacity() {
            let temp = TempDir::new();
            run(
                &temp.0,
                "app",
                1,
                StorageRequestKind::Set {
                    key: "owned".into(),
                    value: vec![7; 4096],
                },
            );
            let estimate = run(
                &temp.0,
                "app",
                2,
                StorageRequestKind::Estimate,
            );
            let StorageResult::Estimate(estimate) = estimate else {
                panic!("estimate returned the wrong result kind")
            };
            assert!(estimate.usage >= 4096);
            assert!(estimate.quota >= estimate.usage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_bookkeeping_restores_namespace_and_rejects_wrong_op() {
        let mut state = StorageState::default();
        let first = state.begin("one".into(), StorageOp::Get);
        let second = state.begin("two".into(), StorageOp::Delete);
        assert_ne!(first, second);

        let response = state
            .finish(StorageWorkerResponse::new(
                first,
                StorageOp::Get,
                Ok(StorageResult::Value(None)),
            ))
            .unwrap();
        assert_eq!(response.namespace, "one");
        assert_eq!(response.op, StorageOp::Get);

        let response = state
            .finish(StorageWorkerResponse::new(
                second,
                StorageOp::Set,
                Ok(StorageResult::Unit),
            ))
            .unwrap();
        assert_eq!(response.namespace, "two");
        assert_eq!(response.op, StorageOp::Delete);
        assert!(matches!(response.result, Err(StorageError::Protocol(_))));
        assert!(state
            .finish(StorageWorkerResponse::new(
                first,
                StorageOp::Get,
                Ok(StorageResult::Value(None))
            ))
            .is_none());
    }

    #[test]
    fn namespace_validation_is_path_safe() {
        assert!(validate_namespace("com.example_app-1").is_ok());
        for invalid in ["", ".", "..", "../app", "app/name", "app name", "☃"] {
            assert!(validate_namespace(invalid).is_err(), "{invalid:?}");
        }
        assert!(validate_namespace(&"a".repeat(65)).is_err());
    }
}
