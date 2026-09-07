use crate::protocol::{error, SessionLocation};
use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
type Handle = *mut c_void;
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}
#[repr(C)]
struct Acl {
    revision: u8,
    reserved: u8,
    size: u16,
    count: u16,
    reserved2: u16,
}
#[repr(C)]
struct Ace {
    kind: u8,
    flags: u8,
    size: u16,
    mask: u32,
    sid: u32,
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        data: *mut c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: *const c_void, text: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn GetNamedSecurityInfoW(
        path: *const u16,
        object: u32,
        information: u32,
        owner: *mut *mut c_void,
        group: *mut *mut c_void,
        dacl: *mut *mut Acl,
        sacl: *mut *mut c_void,
        descriptor: *mut *mut c_void,
    ) -> u32;
    fn GetAce(acl: *const Acl, index: u32, ace: *mut *mut c_void) -> i32;
    fn EqualSid(a: *const c_void, b: *const c_void) -> i32;
    fn IsWellKnownSid(sid: *const c_void, kind: u32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const SecurityAttributes,
        creation: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_buffer: u32,
        in_buffer: u32,
        timeout: u32,
        security: *const SecurityAttributes,
    ) -> Handle;
    fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> i32;
    fn CreateEventW(
        security: *const SecurityAttributes,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn CancelIoEx(handle: Handle, overlapped: *mut c_void) -> i32;
    fn GetOverlappedResult(
        handle: Handle,
        overlapped: *mut c_void,
        count: *mut u32,
        wait: i32,
    ) -> i32;
    fn CreateDirectoryW(path: *const u16, security: *const SecurityAttributes) -> i32;
    fn GetStdHandle(which: u32) -> Handle;
    fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetFinalPathNameByHandleW(handle: Handle, path: *mut u16, size: u32, flags: u32) -> u32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}
#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(algorithm: Handle, buffer: *mut u8, length: u32, flags: u32) -> i32;
}
pub fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Identity {
    token: Handle,
    data: Vec<usize>,
}
impl Identity {
    fn current() -> io::Result<Self> {
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), 8, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut result = Self {
            token,
            data: Vec::new(),
        };
        let mut length = 0;
        unsafe {
            GetTokenInformation(token, 1, std::ptr::null_mut(), 0, &mut length);
        }
        if !(8..=16384).contains(&length) {
            return Err(io::Error::other("Invalid current-user SID size"));
        }
        result
            .data
            .resize((length as usize).div_ceil(std::mem::size_of::<usize>()), 0);
        if unsafe {
            GetTokenInformation(
                token,
                1,
                result.data.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(result)
    }
    fn sid(&self) -> *const c_void {
        self.data[0] as *const c_void
    }
    fn descriptor(&self) -> io::Result<Local> {
        let mut text = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let _text = Local(text.cast());
        let mut n = 0;
        while n < 256 && unsafe { *text.add(n) } != 0 {
            n += 1;
        }
        if n == 256 {
            return Err(io::Error::other("Invalid current-user SID string"));
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(text, n) })
            .map_err(io::Error::other)?;
        let sddl = wide(std::ffi::OsStr::new(&format!(
            "O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)"
        )));
        let mut descriptor = std::ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Local(descriptor))
    }
}
impl Drop for Identity {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.token);
        }
    }
}
fn no_reparse(path: &Path) -> io::Result<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_attributes() & 0x400 != 0 {
        return Err(io::Error::other("Screen state cannot be a reparse point"));
    }
    Ok(metadata)
}
fn validate_acl(path: &Path) -> io::Result<()> {
    no_reparse(path)?;
    let identity = Identity::current()?;
    let path = wide(path.as_os_str());
    let mut owner = std::ptr::null_mut();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let code = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            1,
            1 | 4,
            &mut owner,
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let _descriptor = Local(descriptor);
    if owner.is_null() || acl.is_null() || unsafe { EqualSid(owner, identity.sid()) } == 0 {
        return Err(io::Error::other(
            "Screen state must be owned by the current Windows user with a private ACL",
        ));
    }
    let count = unsafe { (*acl).count };
    if count > 64 {
        return Err(io::Error::other("Screen state ACL exceeds its bound"));
    }
    let mut current_allowed = false;
    for index in 0..count {
        let mut pointer = std::ptr::null_mut();
        if unsafe { GetAce(acl, index.into(), &mut pointer) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = unsafe { &*(pointer as *const Ace) };
        // Accept only basic allow/deny ACEs; object/callback permissions cannot
        // be established as private by this small validator.
        if !matches!(ace.kind, 0 | 1) || ace.size < 12 {
            return Err(io::Error::other("Unsupported screen state ACL"));
        }
        if ace.kind == 1 {
            continue;
        }
        let sid = std::ptr::addr_of!(ace.sid).cast();
        let own = unsafe { EqualSid(sid, identity.sid()) } != 0;
        if !own && unsafe { IsWellKnownSid(sid, 22) } == 0 {
            return Err(io::Error::other(
                "Screen state grants access outside the current user and SYSTEM",
            ));
        }
        current_allowed |= own;
    }
    if !current_allowed {
        return Err(io::Error::other(
            "Screen state ACL does not grant current-user access",
        ));
    }
    Ok(())
}
pub fn private_directory(path: &Path, create: bool) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Screen state path must be absolute".into());
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
            let parent = path.parent().ok_or("Screen directory has no parent")?;
            if !parent.exists() {
                private_directory(parent, true)?;
            }
            no_reparse(parent).map_err(error)?;
            let identity = Identity::current().map_err(error)?;
            let descriptor = identity.descriptor().map_err(error)?;
            let attributes = SecurityAttributes {
                length: std::mem::size_of::<SecurityAttributes>() as u32,
                descriptor: descriptor.0,
                inherit: 0,
            };
            if unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &attributes) } == 0
                && io::Error::last_os_error().raw_os_error() != Some(183)
            {
                return Err(io::Error::last_os_error().to_string());
            }
        }
        Err(e) => return Err(e.to_string()),
    }
    if !no_reparse(path).map_err(error)?.is_dir() {
        return Err("Screen state is not a directory".into());
    }
    validate_acl(path).map_err(error)
}
pub fn create_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let identity = Identity::current().map_err(error)?;
    let descriptor = identity.descriptor().map_err(error)?;
    let attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: descriptor.0,
        inherit: 0,
    };
    let raw = unsafe {
        CreateFileW(
            wide(path.as_os_str()).as_ptr(),
            0x40000000,
            0,
            &attributes,
            1,
            0x80 | 0x00200000,
            std::ptr::null_mut(),
        )
    };
    if raw as isize == -1 {
        return Err(io::Error::last_os_error().to_string());
    }
    let mut file = unsafe { File::from_raw_handle(raw) };
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(error)
}
pub fn read_private(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let metadata = no_reparse(path).map_err(error)?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("Screen metadata must be a bounded regular file".into());
    }
    validate_acl(path).map_err(error)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(0x00200000)
        .open(path)
        .map_err(error)?;
    if file.metadata().map_err(error)?.file_attributes() & 0x400 != 0 {
        return Err("Screen metadata changed to a reparse point".into());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > limit {
        return Err("Screen metadata exceeds its bound".into());
    }
    Ok(bytes)
}
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 16384 {
        return Err("Screen metadata exceeds16KiB".into());
    }
    if path.exists() {
        read_private(path, 16384)?;
    }
    let parent = path.parent().ok_or("Missing screen metadata parent")?;
    let temp = parent.join(format!(".screen-{}.tmp", random_token()?));
    create_private(&temp, bytes)?;
    let result = unsafe {
        MoveFileExW(
            wide(temp.as_os_str()).as_ptr(),
            wide(path.as_os_str()).as_ptr(),
            1 | 8,
        )
    };
    if result == 0 {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(temp);
        return Err(error.to_string());
    }
    Ok(())
}
pub fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    if unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 32, 2) } < 0 {
        return Err("Windows cryptographic entropy unavailable".into());
    }
    Ok(crate::digest::hex(&bytes))
}
/// Exclusive sharing is a lease on the file object, so inherited handles keep
/// it claimed even if the launcher dies before the daemon reaches startup.
pub struct SessionLock {
    file: File,
}
impl SessionLock {
    pub fn acquire(location: &SessionLocation) -> Result<Option<Self>, String> {
        let path = location.lock_path();
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .share_mode(0)
            .custom_flags(0x00200000)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.raw_os_error() == Some(32) => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if file.metadata().map_err(error)?.file_attributes() & 0x400 != 0 {
            return Err("Invalid screen session lease".into());
        }
        Ok(Some(Self { file }))
    }
    pub fn inherit(&self, inherit: bool) -> Result<i32, String> {
        if unsafe { SetHandleInformation(self.file.as_raw_handle(), 1, inherit as u32) } == 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        Ok(0)
    }
    pub fn inherited_file(&self) -> Result<File, String> {
        self.file.try_clone().map_err(error)
    }
    pub unsafe fn from_inherited(location: &SessionLocation, fd: i32) -> Result<Self, String> {
        if fd != 0 {
            return Err("Invalid inherited Windows lease".into());
        }
        let mut raw = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetStdHandle(-10i32 as u32),
                GetCurrentProcess(),
                &mut raw,
                0,
                0,
                2,
            )
        } == 0
        {
            return Err(io::Error::last_os_error().to_string());
        }
        let file = unsafe { File::from_raw_handle(raw) };
        let mut name = vec![0u16; 32768];
        let n = unsafe { GetFinalPathNameByHandleW(raw, name.as_mut_ptr(), name.len() as u32, 0) };
        if n == 0 || n as usize >= name.len() {
            return Err("Inherited screen lease path unavailable".into());
        }
        let actual = String::from_utf16(&name[..n as usize]).map_err(error)?;
        let expected = location.lock_path().to_string_lossy().into_owned();
        if actual.trim_start_matches("\\\\?\\").to_lowercase()
            != expected.trim_start_matches("\\\\?\\").to_lowercase()
        {
            return Err("Inherited lease belongs to another screen session".into());
        }
        let lock = Self { file };
        lock.inherit(false)?;
        Ok(lock)
    }
}

/// Host end supports overlapped I/O; the ConPTY end is synchronous. The name
/// is private and unguessable, and the kernel rejects remote pipe clients.
pub fn pipe_pair(inbound: bool) -> io::Result<(File, File)> {
    let identity = Identity::current()?;
    let descriptor = identity.descriptor()?;
    let attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: descriptor.0,
        inherit: 0,
    };
    let name = wide(std::ffi::OsStr::new(&format!(
        "\\\\.\\pipe\\makepad-screen-{}",
        random_token().map_err(io::Error::other)?
    )));
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            if inbound {
                1 | 0x40000000 | 0x00080000
            } else {
                2 | 0x40000000 | 0x00080000
            },
            8,
            1,
            65536,
            65536,
            0,
            &attributes,
        )
    };
    if raw as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    let host = unsafe { File::from_raw_handle(raw) };
    let remote = unsafe {
        CreateFileW(
            name.as_ptr(),
            if inbound { 0x40000000 } else { 0x80000000 },
            0,
            &attributes,
            3,
            0,
            std::ptr::null_mut(),
        )
    };
    if remote as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    let child = unsafe { File::from_raw_handle(remote) };
    #[repr(C)]
    struct Connection {
        internal: usize,
        high: usize,
        offset: [u32; 2],
        event: Handle,
    }
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let mut connection = Connection {
        internal: 0,
        high: 0,
        offset: [0; 2],
        event: event.as_raw_handle(),
    };
    let operation = std::ptr::addr_of_mut!(connection).cast();
    if unsafe { ConnectNamedPipe(raw, operation) } == 0 {
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(535) => {} // Our synchronous client already connected.
            Some(997) => {
                let completed = unsafe { WaitForSingleObject(event.as_raw_handle(), 2000) } == 0;
                if !completed {
                    unsafe {
                        CancelIoEx(raw, operation);
                    }
                }
                let mut count = 0;
                let result = unsafe { GetOverlappedResult(raw, operation, &mut count, 1) };
                if !completed {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Private ConPTY pipe connection timed out",
                    ));
                }
                if result == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            _ => return Err(error),
        }
    }
    Ok((host, child))
}
