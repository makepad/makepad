use {
    makepad_android_state::get_activity,
    makepad_jni_sys as jni_sys,
    crate::h264_packets,
    makepad_platform::{
        call_bool_method, call_void_method, makepad_error_log::error,
        os::linux::android::android_jni, CameraFrameOwned, CameraFrameRef, EncodedVideoPacketRef,
        MediaVideoEncoder, VideoBitstreamFormat, VideoCodec, VideoEncodeError, VideoEncoderConfig,
        VideoOutputFn, VideoQueuePolicy,
    },
    std::{
        collections::{HashMap, VecDeque},
        ffi::CString,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc, Condvar, Mutex, OnceLock,
        },
    },
};

const ANDROID_BUFFER_FLAG_KEY_FRAME: i32 = 1;
const ANDROID_BUFFER_FLAG_CODEC_CONFIG: i32 = 2;
const ANDROID_BUFFER_FLAG_END_OF_STREAM: i32 = 4;

#[derive(Clone, Copy)]
struct JavaGlobalRef(jni_sys::jobject);
unsafe impl Send for JavaGlobalRef {}
unsafe impl Sync for JavaGlobalRef {}

struct SharedQueue {
    queue: Mutex<VecDeque<CameraFrameOwned>>,
    condvar: Condvar,
}

struct AndroidH264OutputState {
    output: VideoOutputFn,
    config_id: u32,
    last_emitted_config_id: Option<u32>,
    active_config_annexb: Vec<u8>,
    nal_len_size: usize,
}

static NEXT_ENCODER_ID: AtomicU64 = AtomicU64::new(1);
static OUTPUT_REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<Mutex<AndroidH264OutputState>>>>> =
    OnceLock::new();

fn output_registry() -> &'static Mutex<HashMap<u64, Arc<Mutex<AndroidH264OutputState>>>> {
    OUTPUT_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe fn clear_pending_jni_exception(env: *mut jni_sys::JNIEnv, context: &str) -> bool {
    if (**env).ExceptionCheck.unwrap()(env) == 0 {
        return false;
    }
    (**env).ExceptionDescribe.unwrap()(env);
    (**env).ExceptionClear.unwrap()(env);
    error!("android h264 encoder jni failure: {}", context);
    true
}

unsafe fn load_app_class(
    env: *mut jni_sys::JNIEnv,
    activity: jni_sys::jobject,
    class_name_dot: &str,
) -> jni_sys::jclass {
    let activity_class = (**env).GetObjectClass.unwrap()(env, activity);
    if activity_class.is_null() {
        return std::ptr::null_mut();
    }

    let get_class_loader = (**env).GetMethodID.unwrap()(
        env,
        activity_class,
        b"getClassLoader\0".as_ptr() as _,
        b"()Ljava/lang/ClassLoader;\0".as_ptr() as _,
    );
    if get_class_loader.is_null() || clear_pending_jni_exception(env, "Activity.getClassLoader") {
        (**env).DeleteLocalRef.unwrap()(env, activity_class);
        return std::ptr::null_mut();
    }

    let class_loader = (**env).CallObjectMethod.unwrap()(env, activity, get_class_loader);
    if class_loader.is_null() || clear_pending_jni_exception(env, "Activity.getClassLoader call") {
        (**env).DeleteLocalRef.unwrap()(env, activity_class);
        return std::ptr::null_mut();
    }

    let class_loader_class = (**env).GetObjectClass.unwrap()(env, class_loader);
    if class_loader_class.is_null() {
        (**env).DeleteLocalRef.unwrap()(env, class_loader);
        (**env).DeleteLocalRef.unwrap()(env, activity_class);
        return std::ptr::null_mut();
    }

    let load_class = (**env).GetMethodID.unwrap()(
        env,
        class_loader_class,
        b"loadClass\0".as_ptr() as _,
        b"(Ljava/lang/String;)Ljava/lang/Class;\0".as_ptr() as _,
    );
    if load_class.is_null() || clear_pending_jni_exception(env, "ClassLoader.loadClass") {
        (**env).DeleteLocalRef.unwrap()(env, class_loader_class);
        (**env).DeleteLocalRef.unwrap()(env, class_loader);
        (**env).DeleteLocalRef.unwrap()(env, activity_class);
        return std::ptr::null_mut();
    }

    let name = CString::new(class_name_dot).unwrap();
    let jname = (**env).NewStringUTF.unwrap()(env, name.as_ptr());
    if jname.is_null() {
        (**env).DeleteLocalRef.unwrap()(env, class_loader_class);
        (**env).DeleteLocalRef.unwrap()(env, class_loader);
        (**env).DeleteLocalRef.unwrap()(env, activity_class);
        return std::ptr::null_mut();
    }

    let class_obj = (**env).CallObjectMethod.unwrap()(env, class_loader, load_class, jname);
    let had_error = clear_pending_jni_exception(env, class_name_dot);

    (**env).DeleteLocalRef.unwrap()(env, jname as jni_sys::jobject);
    (**env).DeleteLocalRef.unwrap()(env, class_loader_class);
    (**env).DeleteLocalRef.unwrap()(env, class_loader);
    (**env).DeleteLocalRef.unwrap()(env, activity_class);

    if had_error || class_obj.is_null() {
        return std::ptr::null_mut();
    }

    class_obj as jni_sys::jclass
}

unsafe fn new_h264_encoder_java_object(
    env: *mut jni_sys::JNIEnv,
    activity: jni_sys::jobject,
    encoder_id: jni_sys::jlong,
) -> jni_sys::jobject {
    let class = load_app_class(env, activity, "dev.makepad.android.H264Encoder");
    if class.is_null() {
        return std::ptr::null_mut();
    }

    let sig = CString::new("(Landroid/app/Activity;J)V").unwrap();
    let constructor = (**env).GetMethodID.unwrap()(
        env,
        class,
        b"<init>\0".as_ptr() as _,
        sig.as_ptr(),
    );
    if constructor.is_null() || clear_pending_jni_exception(env, "H264Encoder.<init>") {
        (**env).DeleteLocalRef.unwrap()(env, class as jni_sys::jobject);
        return std::ptr::null_mut();
    }

    let obj = (**env).NewObject.unwrap()(env, class, constructor, activity, encoder_id);
    let had_error = clear_pending_jni_exception(env, "new H264Encoder");
    (**env).DeleteLocalRef.unwrap()(env, class as jni_sys::jobject);

    if had_error {
        return std::ptr::null_mut();
    }

    obj
}

pub fn on_java_h264_packet(encoder_id: u64, pts_us: i64, flags: i32, data: Vec<u8>) {
    let output_state = {
        let reg = output_registry().lock().unwrap();
        reg.get(&encoder_id).cloned()
    };
    let Some(output_state) = output_state else {
        return;
    };

    let mut st = output_state.lock().unwrap();
    let pts_ns = pts_us.max(0) as u64 * 1000;
    let is_config = (flags & ANDROID_BUFFER_FLAG_CODEC_CONFIG) != 0;
    let is_eos = (flags & ANDROID_BUFFER_FLAG_END_OF_STREAM) != 0;

    if is_config {
        let mut config_annexb = if h264_packets::starts_with_annexb(&data) {
            data
        } else if let Some((sps, pps, nal_len_size)) = h264_packets::avcc_config_to_sps_pps(&data) {
            st.nal_len_size = nal_len_size;
            h264_packets::sps_pps_to_annexb(&sps, &pps)
        } else {
            data
        };

        if config_annexb.is_empty() {
            return;
        }

        if !h264_packets::starts_with_annexb(&config_annexb) {
            if let Some(converted) = h264_packets::avcc_sample_to_annexb(&config_annexb, st.nal_len_size)
            {
                config_annexb = converted;
            }
        }

        if st.active_config_annexb != config_annexb {
            st.config_id = st.config_id.saturating_add(1);
            st.active_config_annexb = config_annexb.clone();
        }

        let config_id = st.config_id;
        (st.output)(EncodedVideoPacketRef {
            codec: VideoCodec::H264,
            format: VideoBitstreamFormat::AnnexB,
            pts_ns,
            dts_ns: None,
            is_key: false,
            is_config: true,
            is_eos: false,
            config_id,
            data: &config_annexb,
        });
        st.last_emitted_config_id = Some(config_id);
        return;
    }

    let mut packet_format = if h264_packets::starts_with_annexb(&data) {
        VideoBitstreamFormat::AnnexB
    } else {
        VideoBitstreamFormat::Avcc
    };
    let mut packet_data = data;

    if packet_format == VideoBitstreamFormat::Avcc {
        if let Some(annexb) = h264_packets::avcc_sample_to_annexb(&packet_data, st.nal_len_size) {
            packet_data = annexb;
            packet_format = VideoBitstreamFormat::AnnexB;
        }
    }

    let mut is_key = (flags & ANDROID_BUFFER_FLAG_KEY_FRAME) != 0;
    if !is_key && packet_format == VideoBitstreamFormat::AnnexB {
        is_key = h264_packets::contains_idr_annexb(&packet_data);
    }

    let config_id = st.config_id;
    if is_key && !st.active_config_annexb.is_empty() && st.last_emitted_config_id != Some(config_id)
    {
        let cfg = st.active_config_annexb.clone();
        (st.output)(EncodedVideoPacketRef {
            codec: VideoCodec::H264,
            format: VideoBitstreamFormat::AnnexB,
            pts_ns,
            dts_ns: None,
            is_key: false,
            is_config: true,
            is_eos: false,
            config_id,
            data: &cfg,
        });
        st.last_emitted_config_id = Some(config_id);
    }

    (st.output)(EncodedVideoPacketRef {
        codec: VideoCodec::H264,
        format: packet_format,
        pts_ns,
        dts_ns: None,
        is_key,
        is_config: false,
        is_eos,
        config_id,
        data: &packet_data,
    });
}

pub fn on_java_h264_error(encoder_id: u64, message: String) {
    error!("android h264 encoder {} error: {}", encoder_id, message);
}

pub struct AndroidH264Encoder {
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
    queue_policy: VideoQueuePolicy,
    queue_capacity: usize,
    java_encoder: JavaGlobalRef,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    output_state: Arc<Mutex<AndroidH264OutputState>>,
    encoder_id: u64,
}

unsafe impl Send for AndroidH264Encoder {}
unsafe impl Sync for AndroidH264Encoder {}

impl AndroidH264Encoder {
    pub fn start(config: VideoEncoderConfig, output: VideoOutputFn) -> Option<Self> {
        if config.codec != VideoCodec::H264 {
            return None;
        }
        if config.width == 0 || config.height == 0 || config.fps_num == 0 {
            return None;
        }

        let encoder_id = NEXT_ENCODER_ID.fetch_add(1, Ordering::Relaxed);
        let output_state = Arc::new(Mutex::new(AndroidH264OutputState {
            output,
            config_id: 0,
            last_emitted_config_id: None,
            active_config_annexb: Vec::new(),
            nal_len_size: 4,
        }));

        output_registry()
            .lock()
            .unwrap()
            .insert(encoder_id, output_state.clone());

        let queue = Arc::new(SharedQueue {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        });
        let running = Arc::new(AtomicBool::new(true));

        let env = unsafe { android_jni::attach_jni_env() };
        let activity = get_activity();
        if activity.is_null() {
            output_registry().lock().unwrap().remove(&encoder_id);
            return None;
        }

        let local_encoder =
            unsafe { new_h264_encoder_java_object(env, activity, encoder_id as jni_sys::jlong) };

        if local_encoder.is_null() {
            output_registry().lock().unwrap().remove(&encoder_id);
            return None;
        }

        let java_encoder = unsafe { (**env).NewGlobalRef.unwrap()(env, local_encoder) };
        unsafe {
            (**env).DeleteLocalRef.unwrap()(env, local_encoder);
        }

        if java_encoder.is_null() {
            output_registry().lock().unwrap().remove(&encoder_id);
            return None;
        }
        let java_encoder = JavaGlobalRef(java_encoder);

        let running_clone = running.clone();
        let queue_clone = queue.clone();
        let java_encoder_thread = java_encoder;
        let cfg = config;
        let worker = std::thread::Builder::new()
            .name("android-h264-encoder".to_string())
            .spawn(move || {
                worker_loop(cfg, running_clone, queue_clone, java_encoder_thread);
            })
            .ok()?;

        Some(Self {
            running,
            queue,
            queue_policy: config.queue_policy,
            queue_capacity: config.queue_capacity.max(1),
            java_encoder,
            worker: Mutex::new(Some(worker)),
            output_state,
            encoder_id,
        })
    }

    pub fn stop(&self) {
        if self.running.swap(false, Ordering::SeqCst) {
            self.queue.condvar.notify_all();
            if let Some(worker) = self.worker.lock().unwrap().take() {
                let _ = worker.join();
            }

            output_registry().lock().unwrap().remove(&self.encoder_id);

            let mut st = self.output_state.lock().unwrap();
            let eos_config_id = st.config_id;
            (st.output)(EncodedVideoPacketRef {
                codec: VideoCodec::H264,
                format: VideoBitstreamFormat::AnnexB,
                pts_ns: 0,
                dts_ns: None,
                is_key: false,
                is_config: false,
                is_eos: true,
                config_id: eos_config_id,
                data: &[],
            });
        }
    }
}

impl MediaVideoEncoder for AndroidH264Encoder {
    fn push_frame(&self, frame: CameraFrameRef<'_>) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }

        let mut owned = CameraFrameOwned::default();
        if !owned.convert_to_i420(frame) {
            return;
        }

        let mut q = self.queue.queue.lock().unwrap();
        match self.queue_policy {
            VideoQueuePolicy::LatestWins => {
                if q.len() >= self.queue_capacity {
                    q.pop_front();
                }
            }
        }
        q.push_back(owned);
        self.queue.condvar.notify_one();
    }

    fn request_keyframe(&self) -> Result<(), VideoEncodeError> {
        if !self.running.load(Ordering::Relaxed) {
            return Err(VideoEncodeError::EncoderNotStarted);
        }
        let env = unsafe { android_jni::attach_jni_env() };
        unsafe {
            call_void_method!(env, self.java_encoder.0, "requestKeyframe", "()V");
        }
        Ok(())
    }

    fn stop(&self) {
        AndroidH264Encoder::stop(self);
    }
}

impl Drop for AndroidH264Encoder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker_loop(
    config: VideoEncoderConfig,
    running: Arc<AtomicBool>,
    queue: Arc<SharedQueue>,
    java_encoder: JavaGlobalRef,
) {
    let env = unsafe { android_jni::attach_jni_env() };

    let keyint_seconds = if config.fps_num == 0 {
        1
    } else {
        ((config.keyint.max(1) as u32 + config.fps_num - 1) / config.fps_num).max(1)
    };

    let started = unsafe {
        call_bool_method!(
            env,
            java_encoder.0,
            "start",
            "(IIIII)Z",
            config.width as jni_sys::jint,
            config.height as jni_sys::jint,
            config.fps_num as jni_sys::jint,
            config.target_bitrate as jni_sys::jint,
            keyint_seconds as jni_sys::jint
        ) != 0
    };

    if !started {
        error!("android h264 encoder start failed");
        running.store(false, Ordering::SeqCst);
        unsafe {
            call_void_method!(env, java_encoder.0, "stop", "()V");
            (**env).DeleteGlobalRef.unwrap()(env, java_encoder.0);
        }
        return;
    }

    loop {
        let frame = {
            let mut guard = queue.queue.lock().unwrap();
            while running.load(Ordering::Relaxed) && guard.is_empty() {
                guard = queue.condvar.wait(guard).unwrap();
            }
            guard.pop_front()
        };

        if let Some(frame) = frame {
            if frame.width as u32 != config.width || frame.height as u32 != config.height {
                continue;
            }
            if frame.plane_count < 3 {
                continue;
            }

            let y = &frame.planes[0].bytes;
            let u = &frame.planes[1].bytes;
            let v = &frame.planes[2].bytes;
            if y.is_empty() || u.is_empty() || v.is_empty() {
                continue;
            }

            // Android MediaCodec with COLOR_FormatYUV420Flexible expects NV12
            // (Y plane + interleaved UV), not I420 (Y + U + V separate planes).
            let cw = (frame.width as u32 + 1) / 2;
            let ch = (frame.height as u32 + 1) / 2;
            let uv_len = (cw * ch * 2) as usize;
            let data_len = y.len() + uv_len;
            let data = unsafe { (**env).NewByteArray.unwrap()(env, data_len as i32) };
            if data.is_null() {
                continue;
            }

            let mut packed = Vec::with_capacity(data_len);
            packed.extend_from_slice(y);
            // Interleave U and V into NV12 UV plane
            let uv_samples = (cw * ch) as usize;
            for i in 0..uv_samples {
                packed.push(u.get(i).copied().unwrap_or(128));
                packed.push(v.get(i).copied().unwrap_or(128));
            }

            unsafe {
                (**env).SetByteArrayRegion.unwrap()(
                    env,
                    data,
                    0,
                    packed.len() as i32,
                    packed.as_ptr() as *const jni_sys::jbyte,
                );

                call_void_method!(
                    env,
                    java_encoder.0,
                    "queueFrame",
                    "([BJ)V",
                    data,
                    (frame.timestamp_ns / 1000) as jni_sys::jlong
                );

                (**env).DeleteLocalRef.unwrap()(env, data as jni_sys::jobject);
            }
            continue;
        }

        if !running.load(Ordering::Relaxed) {
            break;
        }
    }

    unsafe {
        call_void_method!(env, java_encoder.0, "stop", "()V");
        (**env).DeleteGlobalRef.unwrap()(env, java_encoder.0);
    }
}
