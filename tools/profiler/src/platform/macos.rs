use crate::{
    CaptureConfig, CaptureHeader, CaptureSink, LoadedImage, ProcessSample, ProfilerError,
    ThreadSample,
};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::slice;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const KERN_SUCCESS: kern_return_t = 0;
const KERN_INVALID_ARGUMENT: kern_return_t = 4;
const TASK_DYLD_INFO: task_flavor_t = 17;
const THREAD_BASIC_INFO: thread_flavor_t = 3;
const THREAD_IDENTIFIER_INFO: thread_flavor_t = 4;
const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
const MAX_C_STRING_BYTES: usize = 4096;
const FRAME_POINTER_ALIGNMENT: u64 = 8;
const MAX_FRAME_DELTA: u64 = 16 * 1024 * 1024;
const TASK_READY_TIMEOUT: Duration = Duration::from_millis(500);
const TASK_READY_POLL: Duration = Duration::from_millis(5);

#[cfg(target_arch = "aarch64")]
const THREAD_STATE_FLAVOR: thread_state_flavor_t = 6;
#[cfg(target_arch = "aarch64")]
const THREAD_STATE_COUNT: mach_msg_type_number_t =
    (size_of::<arm_thread_state64_t>() / size_of::<u32>()) as mach_msg_type_number_t;

#[cfg(target_arch = "x86_64")]
const THREAD_STATE_FLAVOR: thread_state_flavor_t = 4;
#[cfg(target_arch = "x86_64")]
const THREAD_STATE_COUNT: mach_msg_type_number_t =
    (size_of::<x86_thread_state64_t>() / size_of::<u32>()) as mach_msg_type_number_t;

#[allow(non_camel_case_types)]
type kern_return_t = i32;
#[allow(non_camel_case_types)]
type mach_port_t = u32;
#[allow(non_camel_case_types)]
type task_t = mach_port_t;
#[allow(non_camel_case_types)]
type task_name_t = mach_port_t;
#[allow(non_camel_case_types)]
type task_read_t = mach_port_t;
#[allow(non_camel_case_types)]
type task_inspect_t = mach_port_t;
#[allow(non_camel_case_types)]
type vm_map_t = mach_port_t;
#[allow(non_camel_case_types)]
type vm_map_read_t = mach_port_t;
#[allow(non_camel_case_types)]
type thread_act_t = mach_port_t;
#[allow(non_camel_case_types)]
type thread_act_array_t = *mut thread_act_t;
#[allow(non_camel_case_types)]
type thread_flavor_t = natural_t;
#[allow(non_camel_case_types)]
type mach_msg_type_number_t = u32;
#[allow(non_camel_case_types)]
type mach_vm_address_t = u64;
#[allow(non_camel_case_types)]
type mach_vm_size_t = u64;
#[allow(non_camel_case_types)]
type natural_t = u32;
#[allow(non_camel_case_types)]
type integer_t = i32;
#[allow(non_camel_case_types)]
type task_flavor_t = u32;
#[allow(non_camel_case_types)]
type thread_state_flavor_t = i32;
#[allow(non_camel_case_types)]
type thread_state_t = *mut natural_t;
#[allow(non_camel_case_types)]
type task_info_t = *mut integer_t;
#[allow(non_camel_case_types)]
type thread_info_t = *mut integer_t;

#[repr(C)]
#[derive(Clone, Copy)]
struct task_dyld_info_data_t {
    all_image_info_addr: mach_vm_address_t,
    all_image_info_size: mach_vm_size_t,
    all_image_info_format: integer_t,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct dyld_all_image_infos_head {
    version: u32,
    info_array_count: u32,
    info_array: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct dyld_image_info {
    image_load_address: u64,
    image_file_path: u64,
    image_file_mod_date: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct stack_frame_record {
    previous_fp: u64,
    return_address: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct time_value_t {
    seconds: integer_t,
    microseconds: integer_t,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct thread_basic_info_data_t {
    user_time: time_value_t,
    system_time: time_value_t,
    cpu_usage: integer_t,
    policy: integer_t,
    run_state: integer_t,
    flags: integer_t,
    suspend_count: integer_t,
    sleep_time: integer_t,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct thread_identifier_info_data_t {
    thread_id: u64,
    thread_handle: u64,
    dispatch_qaddr: u64,
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct arm_thread_state64_t {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    flags: u32,
    _pad_or_flags: u32,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct x86_thread_state64_t {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rdi: u64,
    rsi: u64,
    rbp: u64,
    rsp: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
    rflags: u64,
    cs: u64,
    fs: u64,
    gs: u64,
}

#[link(name = "System")]
unsafe extern "C" {
    fn mach_task_self() -> mach_port_t;
    fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> kern_return_t;
    fn mach_error_string(err: kern_return_t) -> *const c_char;
    fn task_for_pid(target_tport: task_t, pid: c_int, task: *mut task_t) -> kern_return_t;
    fn task_suspend(target_task: task_read_t) -> kern_return_t;
    fn task_resume(target_task: task_read_t) -> kern_return_t;
    fn task_threads(
        target_task: task_inspect_t,
        act_list: *mut thread_act_array_t,
        act_list_count: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    fn thread_get_state(
        target_act: thread_act_t,
        flavor: thread_state_flavor_t,
        old_state: thread_state_t,
        old_state_count: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    fn thread_info(
        target_act: thread_act_t,
        flavor: thread_flavor_t,
        thread_info_out: thread_info_t,
        thread_info_out_count: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    fn task_info(
        target_task: task_name_t,
        flavor: task_flavor_t,
        task_info_out: task_info_t,
        task_info_out_count: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
    fn mach_vm_read_overwrite(
        target_task: vm_map_read_t,
        address: mach_vm_address_t,
        size: mach_vm_size_t,
        data: mach_vm_address_t,
        outsize: *mut mach_vm_size_t,
    ) -> kern_return_t;
    fn mach_vm_deallocate(
        target_task: vm_map_t,
        address: mach_vm_address_t,
        size: mach_vm_size_t,
    ) -> kern_return_t;
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
}

pub fn capture_while<F>(
    config: &CaptureConfig,
    sink: &mut dyn CaptureSink,
    mut should_continue: F,
) -> Result<(), ProfilerError>
where
    F: FnMut() -> Result<bool, ProfilerError>,
{
    if config.process_id == std::process::id() {
        return Err(ProfilerError::new(
            "self-profiling is not supported on macOS because task suspension would deadlock the sampler; profile a separate child process instead",
        ));
    }

    let task = TaskPort::attach(config.process_id)?;
    wait_until_suspendable(&task, &mut should_continue)?;
    let executable = read_process_path(config.process_id).unwrap_or_default();
    let mut warnings = Vec::new();
    let images = if config.include_images {
        match collect_loaded_images(&task) {
            Ok(images) => images,
            Err(err) => {
                warnings.push(format!("failed to collect loaded images: {}", err));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let header = CaptureHeader {
        schema: "makepad-profiler.capture".to_string(),
        profiler_version: 1,
        platform: "macos".to_string(),
        architecture: target_architecture().to_string(),
        process_id: config.process_id,
        executable,
        start_unix_micros: unix_timestamp_micros()?,
        duration_micros: 0,
        interval_micros: duration_to_micros(config.interval),
        max_frames: config.max_frames as u32,
    };

    sink.set_header(header);
    for warning in warnings {
        sink.push_warning(warning);
    }
    for image in images {
        sink.push_image(image);
    }

    let started = Instant::now();
    let mut next_deadline = Instant::now();
    while should_continue()? {
        let sample_timestamp = duration_to_micros(started.elapsed());
        let sample = collect_process_sample(&task, sample_timestamp, config.max_frames)?;
        sink.push_sample(sample);

        next_deadline = next_deadline + config.interval;
        let now = Instant::now();
        if next_deadline > now {
            std::thread::sleep(next_deadline - now);
        } else {
            next_deadline = now;
        }
    }

    Ok(())
}

fn wait_until_suspendable<F>(task: &TaskPort, should_continue: &mut F) -> Result<(), ProfilerError>
where
    F: FnMut() -> Result<bool, ProfilerError>,
{
    let started = Instant::now();
    loop {
        match suspend_probe(task) {
            Ok(()) => return Ok(()),
            Err(status)
                if status == KERN_INVALID_ARGUMENT && started.elapsed() < TASK_READY_TIMEOUT =>
            {
                if !should_continue()? {
                    return Err(ProfilerError::new(
                        "process exited before the macOS sampler could suspend it",
                    ));
                }
                std::thread::sleep(TASK_READY_POLL);
            }
            Err(status) => {
                return Err(mach_error(
                    status,
                    "task_suspend probe failed while waiting for the target process to become ready"
                        .to_string(),
                ));
            }
        }
    }
}

fn suspend_probe(task: &TaskPort) -> Result<(), kern_return_t> {
    let status = unsafe { task_suspend(task.raw()) };
    if status != KERN_SUCCESS {
        return Err(status);
    }

    let resume_status = unsafe { task_resume(task.raw()) };
    if resume_status != KERN_SUCCESS {
        return Err(resume_status);
    }

    Ok(())
}

struct TaskPort(task_t);

impl TaskPort {
    fn attach(process_id: u32) -> Result<Self, ProfilerError> {
        let mut task = 0;
        let status = unsafe { task_for_pid(mach_task_self(), process_id as c_int, &mut task) };
        if status != KERN_SUCCESS {
            return Err(mach_error(
                status,
                format!("task_for_pid({}) failed", process_id),
            ));
        }
        Ok(Self(task))
    }

    fn raw(&self) -> task_t {
        self.0
    }
}

impl Drop for TaskPort {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                let _ = mach_port_deallocate(mach_task_self(), self.0);
            }
        }
    }
}

struct SuspendedTask<'a> {
    task: &'a TaskPort,
}

impl<'a> SuspendedTask<'a> {
    fn new(task: &'a TaskPort) -> Result<Self, ProfilerError> {
        let status = unsafe { task_suspend(task.raw()) };
        if status != KERN_SUCCESS {
            return Err(mach_error(status, "task_suspend failed".to_string()));
        }
        Ok(Self { task })
    }
}

impl Drop for SuspendedTask<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = task_resume(self.task.raw());
        }
    }
}

struct ThreadList {
    ports: Vec<thread_act_t>,
}

impl ThreadList {
    fn snapshot(task: &TaskPort) -> Result<Self, ProfilerError> {
        let mut array_ptr: thread_act_array_t = ptr::null_mut();
        let mut count = 0;
        let status = unsafe { task_threads(task.raw(), &mut array_ptr, &mut count) };
        if status != KERN_SUCCESS {
            return Err(mach_error(status, "task_threads failed".to_string()));
        }

        let ports = if array_ptr.is_null() || count == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(array_ptr, count as usize) }.to_vec()
        };

        if !array_ptr.is_null() {
            let size = count as u64 * size_of::<thread_act_t>() as u64;
            unsafe {
                let _ = mach_vm_deallocate(mach_task_self(), array_ptr as u64, size);
            }
        }

        Ok(Self { ports })
    }
}

impl Drop for ThreadList {
    fn drop(&mut self) {
        for port in &self.ports {
            unsafe {
                let _ = mach_port_deallocate(mach_task_self(), *port);
            }
        }
    }
}

fn collect_process_sample(
    task: &TaskPort,
    timestamp_micros: u64,
    max_frames: usize,
) -> Result<ProcessSample, ProfilerError> {
    let suspended_at = Instant::now();
    let _suspended = SuspendedTask::new(task)?;
    let threads = ThreadList::snapshot(task)?;
    let mut thread_samples = Vec::with_capacity(threads.ports.len());

    for thread in &threads.ports {
        thread_samples.push(sample_thread(task, *thread, max_frames));
    }

    Ok(ProcessSample {
        timestamp_micros,
        suspend_micros: duration_to_micros(suspended_at.elapsed()),
        threads: thread_samples,
    })
}

fn sample_thread(task: &TaskPort, thread: thread_act_t, max_frames: usize) -> ThreadSample {
    let thread_id = read_thread_identifier(thread).unwrap_or(0);
    let run_state = read_thread_basic_info(thread)
        .map(|info| info.run_state.max(0) as u32)
        .unwrap_or(0);
    match read_thread_registers(thread) {
        Ok(registers) => {
            let (frames, complete, unwind_error) =
                unwind_frame_pointer_chain(task, registers.pc, registers.fp, max_frames);
            ThreadSample {
                thread_port: thread,
                thread_id,
                run_state,
                pc: registers.pc,
                sp: registers.sp,
                fp: registers.fp,
                frames,
                complete,
                error: unwind_error.unwrap_or_default(),
            }
        }
        Err(err) => ThreadSample {
            thread_port: thread,
            thread_id,
            run_state,
            pc: 0,
            sp: 0,
            fp: 0,
            frames: Vec::new(),
            complete: false,
            error: err.to_string(),
        },
    }
}

fn read_thread_basic_info(thread: thread_act_t) -> Result<thread_basic_info_data_t, ProfilerError> {
    let mut info = MaybeUninit::<thread_basic_info_data_t>::zeroed();
    let mut count = (size_of::<thread_basic_info_data_t>() / size_of::<natural_t>())
        as mach_msg_type_number_t;
    let status = unsafe {
        thread_info(
            thread,
            THREAD_BASIC_INFO,
            info.as_mut_ptr().cast::<integer_t>(),
            &mut count,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            format!("thread_info(THREAD_BASIC_INFO) failed for thread {}", thread),
        ));
    }
    Ok(unsafe { info.assume_init() })
}

fn read_thread_identifier(thread: thread_act_t) -> Result<u64, ProfilerError> {
    let mut info = MaybeUninit::<thread_identifier_info_data_t>::zeroed();
    let mut count = (size_of::<thread_identifier_info_data_t>() / size_of::<natural_t>())
        as mach_msg_type_number_t;
    let status = unsafe {
        thread_info(
            thread,
            THREAD_IDENTIFIER_INFO,
            info.as_mut_ptr().cast::<integer_t>(),
            &mut count,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            format!("thread_info(THREAD_IDENTIFIER_INFO) failed for thread {}", thread),
        ));
    }
    Ok(unsafe { info.assume_init() }.thread_id)
}

#[derive(Clone, Copy)]
struct ThreadRegisters {
    pc: u64,
    sp: u64,
    fp: u64,
}

#[cfg(target_arch = "aarch64")]
fn read_thread_registers(thread: thread_act_t) -> Result<ThreadRegisters, ProfilerError> {
    let mut state = MaybeUninit::<arm_thread_state64_t>::zeroed();
    let mut count = THREAD_STATE_COUNT;
    let status = unsafe {
        thread_get_state(
            thread,
            THREAD_STATE_FLAVOR,
            state.as_mut_ptr().cast::<natural_t>(),
            &mut count,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            format!("thread_get_state failed for thread {}", thread),
        ));
    }
    let state = unsafe { state.assume_init() };
    Ok(ThreadRegisters {
        pc: normalize_code_pointer(state.pc),
        sp: normalize_data_pointer(state.sp),
        fp: normalize_data_pointer(state.fp),
    })
}

#[cfg(target_arch = "x86_64")]
fn read_thread_registers(thread: thread_act_t) -> Result<ThreadRegisters, ProfilerError> {
    let mut state = MaybeUninit::<x86_thread_state64_t>::zeroed();
    let mut count = THREAD_STATE_COUNT;
    let status = unsafe {
        thread_get_state(
            thread,
            THREAD_STATE_FLAVOR,
            state.as_mut_ptr().cast::<natural_t>(),
            &mut count,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            format!("thread_get_state failed for thread {}", thread),
        ));
    }
    let state = unsafe { state.assume_init() };
    Ok(ThreadRegisters {
        pc: state.rip,
        sp: state.rsp,
        fp: state.rbp,
    })
}

fn unwind_frame_pointer_chain(
    task: &TaskPort,
    initial_pc: u64,
    initial_fp: u64,
    max_frames: usize,
) -> (Vec<u64>, bool, Option<String>) {
    let mut frames = Vec::with_capacity(max_frames.min(64));
    if initial_pc != 0 {
        frames.push(initial_pc);
    }

    let mut current_fp = initial_fp;
    let mut complete = true;
    let mut unwind_error = None;

    while frames.len() < max_frames && current_fp != 0 {
        if !looks_like_frame_pointer(current_fp) {
            complete = false;
            unwind_error = Some(format!("frame pointer {:#x} is not aligned", current_fp));
            break;
        }

        let frame = match read_pod::<stack_frame_record>(task, current_fp) {
            Ok(frame) => frame,
            Err(err) => {
                complete = false;
                unwind_error = Some(err.to_string());
                break;
            }
        };

        if frame.return_address != 0 {
            frames.push(normalize_code_pointer(frame.return_address));
        }

        let next_fp = normalize_data_pointer(frame.previous_fp);
        if next_fp == 0 {
            break;
        }
        if next_fp <= current_fp {
            complete = false;
            unwind_error = Some(format!(
                "frame pointer chain regressed from {:#x} to {:#x}",
                current_fp, next_fp
            ));
            break;
        }
        if next_fp - current_fp > MAX_FRAME_DELTA {
            complete = false;
            unwind_error = Some(format!(
                "frame pointer jump from {:#x} to {:#x} exceeded {} bytes",
                current_fp, next_fp, MAX_FRAME_DELTA
            ));
            break;
        }
        current_fp = next_fp;
    }

    (frames, complete, unwind_error)
}

fn collect_loaded_images(task: &TaskPort) -> Result<Vec<LoadedImage>, ProfilerError> {
    let _suspended = SuspendedTask::new(task)?;
    let mut dyld_info = MaybeUninit::<task_dyld_info_data_t>::zeroed();
    let mut count =
        (size_of::<task_dyld_info_data_t>() / size_of::<natural_t>()) as mach_msg_type_number_t;
    let status = unsafe {
        task_info(
            task.raw(),
            TASK_DYLD_INFO,
            dyld_info.as_mut_ptr().cast::<integer_t>(),
            &mut count,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            "task_info(TASK_DYLD_INFO) failed".to_string(),
        ));
    }
    let dyld_info = unsafe { dyld_info.assume_init() };
    if dyld_info.all_image_info_addr == 0 {
        return Ok(Vec::new());
    }

    let infos_head = read_pod::<dyld_all_image_infos_head>(task, dyld_info.all_image_info_addr)?;
    if infos_head.info_array == 0 || infos_head.info_array_count == 0 {
        return Ok(Vec::new());
    }

    let image_infos = read_array::<dyld_image_info>(
        task,
        infos_head.info_array,
        infos_head.info_array_count as usize,
    )?;
    let mut images = Vec::with_capacity(image_infos.len());

    for image in image_infos {
        let path = if image.image_file_path == 0 {
            String::new()
        } else {
            read_c_string(task, image.image_file_path, MAX_C_STRING_BYTES).unwrap_or_default()
        };
        images.push(LoadedImage {
            load_address: image.image_load_address,
            file_mod_date: image.image_file_mod_date as u64,
            path,
        });
    }

    Ok(images)
}

fn read_process_path(process_id: u32) -> Option<String> {
    let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let written = unsafe {
        proc_pidpath(
            process_id as c_int,
            buffer.as_mut_ptr().cast::<c_void>(),
            buffer.len() as u32,
        )
    };
    if written <= 0 {
        return None;
    }
    let written = written as usize;
    let bytes = &buffer[..written];
    let cstr = CStr::from_bytes_until_nul(bytes).ok()?;
    Some(cstr.to_string_lossy().into_owned())
}

fn read_pod<T: Copy>(task: &TaskPort, address: u64) -> Result<T, ProfilerError> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut read_size = 0;
    let status = unsafe {
        mach_vm_read_overwrite(
            task.raw(),
            address,
            size_of::<T>() as u64,
            value.as_mut_ptr() as u64,
            &mut read_size,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            format!(
                "mach_vm_read_overwrite failed while reading {} bytes at {:#x}",
                size_of::<T>(),
                address
            ),
        ));
    }
    if read_size != size_of::<T>() as u64 {
        return Err(ProfilerError::new(format!(
            "short read at {:#x}: expected {} bytes, got {}",
            address,
            size_of::<T>(),
            read_size
        )));
    }
    Ok(unsafe { value.assume_init() })
}

fn read_array<T: Copy>(
    task: &TaskPort,
    address: u64,
    count: usize,
) -> Result<Vec<T>, ProfilerError> {
    if count == 0 {
        return Ok(Vec::new());
    }

    let total_size = count
        .checked_mul(size_of::<T>())
        .ok_or_else(|| ProfilerError::new("array read size overflow"))?;
    let mut values = vec![MaybeUninit::<T>::uninit(); count];
    let mut read_size = 0;
    let status = unsafe {
        mach_vm_read_overwrite(
            task.raw(),
            address,
            total_size as u64,
            values.as_mut_ptr() as u64,
            &mut read_size,
        )
    };
    if status != KERN_SUCCESS {
        return Err(mach_error(
            status,
            format!(
                "mach_vm_read_overwrite failed while reading {} entries at {:#x}",
                count, address
            ),
        ));
    }
    if read_size != total_size as u64 {
        return Err(ProfilerError::new(format!(
            "short read at {:#x}: expected {} bytes, got {}",
            address, total_size, read_size
        )));
    }

    let values = unsafe {
        let ptr = values.as_mut_ptr() as *mut T;
        let len = values.len();
        let cap = values.capacity();
        std::mem::forget(values);
        Vec::from_raw_parts(ptr, len, cap)
    };
    Ok(values)
}

fn read_c_string(task: &TaskPort, address: u64, max_bytes: usize) -> Result<String, ProfilerError> {
    let mut bytes = Vec::new();
    let mut cursor = address;
    let chunk_size = 256usize;

    while bytes.len() < max_bytes {
        let remaining = max_bytes - bytes.len();
        let chunk_len = remaining.min(chunk_size);
        let mut chunk = vec![0u8; chunk_len];
        let mut read_size = 0;
        let status = unsafe {
            mach_vm_read_overwrite(
                task.raw(),
                cursor,
                chunk_len as u64,
                chunk.as_mut_ptr() as u64,
                &mut read_size,
            )
        };
        if status != KERN_SUCCESS {
            return Err(mach_error(
                status,
                format!("failed to read C string at {:#x}", cursor),
            ));
        }
        if read_size == 0 {
            break;
        }
        let read_size = read_size as usize;
        chunk.truncate(read_size);
        if let Some(end) = chunk.iter().position(|byte| *byte == 0) {
            bytes.extend_from_slice(&chunk[..end]);
            break;
        }
        cursor += read_size as u64;
        bytes.extend_from_slice(&chunk);
        if read_size < chunk_len {
            break;
        }
    }

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn unix_timestamp_micros() -> Result<u64, ProfilerError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| ProfilerError::new(format!("system clock error: {}", err)))?;
    Ok(duration_to_micros(duration))
}

fn duration_to_micros(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_mul(1_000_000)
        .saturating_add(duration.subsec_micros() as u64)
}

fn target_architecture() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "aarch64"
    }
    #[cfg(target_arch = "x86_64")]
    {
        "x86_64"
    }
}

fn looks_like_frame_pointer(address: u64) -> bool {
    address >= FRAME_POINTER_ALIGNMENT && address % FRAME_POINTER_ALIGNMENT == 0
}

#[cfg(target_arch = "aarch64")]
fn normalize_code_pointer(address: u64) -> u64 {
    address & 0x0000_FFFF_FFFF_FFFF
}

#[cfg(target_arch = "aarch64")]
fn normalize_data_pointer(address: u64) -> u64 {
    address & 0x0000_FFFF_FFFF_FFFF
}

#[cfg(target_arch = "x86_64")]
fn normalize_code_pointer(address: u64) -> u64 {
    address
}

#[cfg(target_arch = "x86_64")]
fn normalize_data_pointer(address: u64) -> u64 {
    address
}

fn mach_error(status: kern_return_t, context: String) -> ProfilerError {
    let message = unsafe {
        let ptr = mach_error_string(status);
        if ptr.is_null() {
            format!("{} (kern_return_t={})", context, status)
        } else {
            format!(
                "{}: {} (kern_return_t={})",
                context,
                CStr::from_ptr(ptr).to_string_lossy(),
                status
            )
        }
    };
    ProfilerError::new(message)
}
