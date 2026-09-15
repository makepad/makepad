// The documented Process Snapshotting API observes only this pinned root's
// handle names. It neither clones the process nor captures memory or threads.
// https://learn.microsoft.com/windows/win32/api/processsnapshot/ne-processsnapshot-pss_capture_flags
#[repr(C)]
struct SnapshotHandleEntry {
    handle: Handle,
    flags: u32,
    object_type: i32,
    capture_time: FileTime,
    attributes: u32,
    access: u32,
    handle_count: u32,
    pointer_count: u32,
    paged_charge: u32,
    nonpaged_charge: u32,
    creation_time: FileTime,
    type_length: u16,
    type_name: *const u16,
    name_length: u16,
    name: *const u16,
    specific: SnapshotSpecific,
}
#[repr(C)]
union SnapshotSpecific {
    process: SnapshotProcess,
    thread: SnapshotThread,
    section: SnapshotSection,
    small: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy)]
struct SnapshotProcess {
    exit: u32,
    peb: Handle,
    affinity: usize,
    priority: i32,
    pid: u32,
    parent: u32,
    flags: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct SnapshotThread {
    exit: u32,
    teb: Handle,
    pid: u32,
    tid: u32,
    affinity: usize,
    priority: i32,
    base_priority: i32,
    start: Handle,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct SnapshotSection {
    base: Handle,
    attributes: u32,
    size: i64,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn PssCaptureSnapshot(process: Handle, flags: u32, context: u32, snapshot: *mut Handle) -> u32;
    fn PssFreeSnapshot(process: Handle, snapshot: Handle) -> u32;
    fn PssQuerySnapshot(snapshot: Handle, class: i32, data: *mut c_void, length: u32) -> u32;
    fn PssWalkMarkerCreate(allocator: *const c_void, marker: *mut Handle) -> u32;
    fn PssWalkMarkerFree(marker: Handle) -> u32;
    fn PssWalkSnapshot(
        snapshot: Handle,
        class: i32,
        marker: Handle,
        entry: *mut c_void,
        length: u32,
    ) -> u32;
    fn GetProcessHandleCount(process: Handle, count: *mut u32) -> i32;
    fn GetFinalPathNameByHandleW(file: Handle, name: *mut u16, length: u32, flags: u32) -> u32;
}
struct HandleSnapshot(Handle);
impl Drop for HandleSnapshot {
    fn drop(&mut self) {
        unsafe {
            PssFreeSnapshot(GetCurrentProcess(), self.0);
        }
    }
}
struct HandleWalk(Handle);
impl Drop for HandleWalk {
    fn drop(&mut self) {
        unsafe {
            PssWalkMarkerFree(self.0);
        }
    }
}
fn snapshot_error(code: u32) -> String {
    format!("Windows cannot prove this root's open conversation files (snapshot error {code}); its session remains running")
}
fn process_sid(process: Handle) -> Result<Vec<usize>, String> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, 8, &mut token) } == 0 {
        return Err("Cannot verify root process ownership".into());
    }
    let token = OwnedHandle(token);
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.0, 1, std::ptr::null_mut(), 0, &mut needed);
    }
    if !(8..=16384).contains(&needed) {
        return Err("Invalid Windows process identity size".into());
    }
    let mut data = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe { GetTokenInformation(token.0, 1, data.as_mut_ptr().cast(), needed, &mut needed) }
        == 0
    {
        return Err("Cannot read root process ownership".into());
    }
    Ok(data)
}
fn nt_file_name(file: &File) -> Result<String, String> {
    let mut name = vec![0u16; 32768];
    // Opened path avoids normalization walking other network path components;
    // NT-volume form can be compared to the kernel's captured object name.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            name.as_mut_ptr(),
            name.len() as u32,
            2 | 8,
        )
    } as usize;
    if length == 0 || length >= name.len() {
        return Err("Windows cannot resolve the conversation's exact file path".into());
    }
    String::from_utf16(&name[..length]).map_err(|_| "Invalid Windows conversation file name".into())
}
fn opened_conversation_files(proof: &ProcessProof, home: &Path) -> Result<Vec<PathBuf>, String> {
    const MAX_HANDLES: u32 = 4096;
    let own = process_sid(unsafe { GetCurrentProcess() })?;
    let target = process_sid(proof.handle.0)?;
    if unsafe { EqualSid(own[0] as *const c_void, target[0] as *const c_void) } == 0 {
        return Err("The root process belongs to another Windows user".into());
    }
    if !proof.still_live() {
        return Err("The root process exited before conversation inspection".into());
    }
    let home = home
        .canonicalize()
        .map_err(|_| "Provider history directory is unavailable")?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(&home)
        .map_err(|_| "Cannot open the provider history directory")?;
    let metadata = directory.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err("Provider history must be a real directory".into());
    }
    let nt_home = nt_file_name(&directory)?;
    if !nt_home.starts_with("\\Device\\HarddiskVolume") {
        return Err(
            "Opened-file conversation proof requires provider history on a local Windows volume"
                .into(),
        );
    }
    let nt_home = format!("{}\\", nt_home.trim_end_matches('\\'));
    let prefix: Vec<u16> = nt_home.encode_utf16().collect();
    let mut count = 0;
    if unsafe { GetProcessHandleCount(proof.handle.0, &mut count) } == 0 || count > MAX_HANDLES {
        return Err("The root handle table is unavailable or exceeds the inspection limit".into());
    }
    let started = Instant::now();
    let mut raw = std::ptr::null_mut();
    let code = unsafe { PssCaptureSnapshot(proof.handle.0, 4 | 8, 0, &mut raw) };
    if code != 0 {
        return Err(snapshot_error(code));
    }
    if raw.is_null() {
        return Err("Windows returned an invalid handle snapshot".into());
    }
    let snapshot = HandleSnapshot(raw);
    let code = unsafe {
        PssQuerySnapshot(
            snapshot.0,
            4,
            std::ptr::addr_of_mut!(count).cast(),
            std::mem::size_of_val(&count) as u32,
        )
    };
    if code != 0 {
        return Err(snapshot_error(code));
    }
    if count > MAX_HANDLES {
        return Err("Root handles changed beyond the bounded inspection limit".into());
    }
    let mut raw = std::ptr::null_mut();
    let code = unsafe { PssWalkMarkerCreate(std::ptr::null(), &mut raw) };
    if code != 0 {
        return Err(snapshot_error(code));
    }
    if raw.is_null() {
        return Err("Windows returned an invalid snapshot walk marker".into());
    }
    let walk = HandleWalk(raw);
    let mut files = Vec::new();
    let mut complete = false;
    for _ in 0..=MAX_HANDLES {
        if started.elapsed() > Duration::from_secs(2) {
            return Err("Root conversation file inspection exceeded its time budget".into());
        }
        let mut entry: SnapshotHandleEntry = unsafe { std::mem::zeroed() };
        let code = unsafe {
            PssWalkSnapshot(
                snapshot.0,
                2,
                walk.0,
                std::ptr::addr_of_mut!(entry).cast(),
                std::mem::size_of_val(&entry) as u32,
            )
        };
        if code == 259 {
            complete = true;
            break;
        }
        if code != 0 {
            return Err(snapshot_error(code));
        }
        if entry.flags & 2 == 0
            || entry.type_name.is_null()
            || entry.type_length != 8
            || entry.name.is_null()
            || entry.name_length % 2 != 0
            || entry.name_length as usize / 2 <= prefix.len()
        {
            continue;
        }
        // The documented pointers stay valid for the walk marker's lifetime.
        let kind = unsafe { std::slice::from_raw_parts(entry.type_name, 4) };
        if kind != [b'F' as u16, b'i' as u16, b'l' as u16, b'e' as u16] {
            continue;
        }
        let name =
            unsafe { std::slice::from_raw_parts(entry.name, entry.name_length as usize / 2) };
        let actual_prefix =
            String::from_utf16(&name[..prefix.len()]).map_err(|_| "Invalid root file name")?;
        if actual_prefix.to_lowercase() != nt_home.to_lowercase() {
            continue;
        }
        let relative =
            String::from_utf16(&name[prefix.len()..]).map_err(|_| "Invalid provider file name")?;
        if relative.contains(':')
            || relative.chars().any(char::is_control)
            || Path::new(&relative)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("Root file escaped its provider history scope".into());
        }
        let path = home.join(relative);
        let Ok(file) = owned_evidence_file(&path) else {
            continue;
        };
        let observed = nt_file_name(&file)?;
        let captured = String::from_utf16(name).map_err(|_| "Invalid root file name")?;
        if observed.to_lowercase() != captured.to_lowercase() {
            return Err("Provider evidence changed identity during inspection".into());
        }
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if metadata.creation_time() > entry.capture_time.ticks() {
            return Err("Provider evidence was replaced after its handle snapshot".into());
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "Opened provider evidence is unavailable")?;
        if !canonical.starts_with(&home) {
            return Err("Opened provider evidence escaped its history directory".into());
        }
        files.push(canonical);
        if files.len() > 256 {
            return Err("Too many open provider files to prove one conversation".into());
        }
    }
    if !complete || !proof.still_live() {
        return Err("The root changed or its handle inventory was incomplete".into());
    }
    files.sort();
    files.dedup();
    Ok(files)
}

impl Backend {
    fn discover_opened(
        &self,
        id: &str,
        info: &SessionInfo,
        supervisor: &SupervisorProof,
        root: &Process,
        initial: &ProcessProof,
        provider: AgentProvider,
        stop: &AtomicBool,
    ) -> Result<ResumeIdentity, String> {
        // Query + duplicate-handle access is requested only on our known root;
        // no privilege adjustment or process memory access is attempted.
        let proof = ProcessProof::open_with_access(root.pid, 0x0400 | 0x0040 | 0x0010_0000)?
            .ok_or("The root exited before its handle inventory could be inspected")?;
        if proof.start != initial.start || proof.program != initial.program {
            return Err("The root process changed before conversation inspection".into());
        }
        self.verify_root_scope(id, info, supervisor, root.pid, &proof, stop)?;
        let home = provider_home(provider)?;
        let cwd = info
            .cwd
            .canonicalize()
            .map_err(|_| "Agent worktree is unavailable")?;
        let open = opened_conversation_files(&proof, &home)?;
        let mut candidates = Vec::new();
        for path in &open {
            if provider == AgentProvider::Codex
                && path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .is_some_and(|v| v.starts_with("rollout-") && v.ends_with(".jsonl"))
            {
                if let Some(row) = evidence_rows(path)?.first() {
                    let payload = row.get("payload").unwrap_or(&Value::Null);
                    if row.get("type").and_then(Value::as_str) == Some("session_meta")
                        && payload.get("source").and_then(Value::as_str) == Some("cli")
                    {
                        if let Some(uuid) = payload
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|value| uuid(value))
                        {
                            candidates.push((uuid.to_owned(), path.clone(), path.clone()));
                        }
                    }
                }
            } else if provider == AgentProvider::Grok {
                for parent in path.ancestors().take(3) {
                    let Some(conversation) = parent
                        .file_name()
                        .and_then(|v| v.to_str())
                        .filter(|value| uuid(value))
                    else {
                        continue;
                    };
                    let summary = parent.join("summary.json");
                    if summary.is_file() {
                        candidates.push((conversation.to_owned(), summary, path.clone()));
                    }
                }
            }
        }
        let mut verified = BTreeMap::new();
        for (conversation, path, opened) in candidates {
            let identity = ResumeIdentity {
                provider,
                conversation_id: conversation,
                cwd: cwd.to_string_lossy().into_owned(),
                evidence_path: path.to_string_lossy().into_owned(),
                program: proof.program.to_string_lossy().into_owned(),
                provider_home: home.to_string_lossy().into_owned(),
                observed_pid: root.pid,
                process_start: format!("windows-filetime:{}", proof.start),
                verified_at_ms: timestamp_ms(),
            };
            if evidence_matches(&identity).is_ok() {
                verified.insert(
                    (
                        identity.conversation_id.clone(),
                        identity.evidence_path.clone(),
                    ),
                    (identity, opened),
                );
            }
        }
        if verified.len() != 1 {
            return Err(
                "Cannot prove one persisted root conversation; its Windows session remains running"
                    .into(),
            );
        }
        let (identity, opened) = verified.into_values().next().unwrap();
        self.verify_root_scope(id, info, supervisor, root.pid, &proof, stop)?;
        evidence_matches(&identity)?;
        // Re-check the exact opened evidence after parsing and helper queries,
        // so a conversation switch cannot silently authorize persistence.
        if !opened_conversation_files(&proof, &home)?.contains(&opened) || !proof.still_live() {
            return Err("The root changed its open conversation during inspection".into());
        }
        self.persist_identity(id, &identity)?;
        Ok(identity)
    }
    fn persist_identity(&self, id: &str, identity: &ResumeIdentity) -> Result<(), String> {
        write_private(
            &self.file(id, ".resume.ron"),
            identity.serialize_ron().as_bytes(),
        )?;
        let mut launch = self
            .launch_record(id)?
            .ok_or("Missing provider launch configuration")?;
        launch.provider = identity.provider;
        launch.program = identity.program.clone();
        write_private(
            &self.file(id, ".provider.ron"),
            launch.serialize_ron().as_bytes(),
        )
    }
}
