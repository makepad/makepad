//! The LLM node's context window and KV-cache precision as runtime settings.
//!
//! `POST /llm/context {"context": 32768, "kv": "q8"}` sets the
//! per-conversation window and the attention KV precision the node's chat
//! model is built with (either field alone is fine); `GET /llm/context` (and
//! `/health`) report them. Both are persisted in the cache dir
//! (`llm-context.json`) so a restart keeps them. They take effect at the next
//! LLM job: the session is rebuilt then (weights are re-read from the page
//! cache; the KV cache and compute buffers are re-planned), after any job in
//! flight has finished. A client applies them at once by sending a one-token
//! chat job.
//!
//! KV precision: `q8` stores K/V as 8-bit blocks (q8_0, ~half the bytes of
//! f16, so about twice the context in the same memory); `f16` is the
//! reference. Default `q8` (`MAKEPAD_ASSET_AI_LLM_KV` overrides the default;
//! the runtime setting overrides both).

use makepad_strict_json::{self as json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

static GENERATION: AtomicU64 = AtomicU64::new(0);
static SETTINGS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

pub fn bump_generation() {
    GENERATION.fetch_add(1, Ordering::Release);
}

/// 0 selects the configured override or automatic sizing.
static REQUESTED: AtomicU32 = AtomicU32::new(0);
/// The window of the session that is loaded now; 0 when none.
static LOADED: AtomicU32 = AtomicU32::new(0);
/// KV precision asked for (`KvPrecision::code`); 0 = never set.
static KV_REQUESTED: AtomicU8 = AtomicU8::new(0);
/// KV precision of the loaded session; 0 when none.
static KV_LOADED: AtomicU8 = AtomicU8::new(0);

pub const MIN_CONTEXT: u32 = 2048;

/// Attention KV-cache precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KvPrecision {
    F16,
    Q8,
}

impl KvPrecision {
    pub const ALL: [KvPrecision; 2] = [KvPrecision::F16, KvPrecision::Q8];

    pub fn slug(self) -> &'static str {
        match self {
            KvPrecision::F16 => "f16",
            KvPrecision::Q8 => "q8",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "f16" | "fp16" | "16" => Some(KvPrecision::F16),
            "q8" | "q8_0" | "8" | "int8" => Some(KvPrecision::Q8),
            _ => None,
        }
    }

    fn code(self) -> u8 {
        match self {
            KvPrecision::F16 => 1,
            KvPrecision::Q8 => 2,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(KvPrecision::F16),
            2 => Some(KvPrecision::Q8),
            _ => None,
        }
    }
}

pub fn requested() -> Option<u32> {
    match REQUESTED.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}

pub fn loaded() -> Option<u32> {
    match LOADED.load(Ordering::Relaxed) {
        0 => None,
        n => Some(n),
    }
}

#[cfg_attr(not(feature = "llm"), allow(dead_code))]
pub(crate) fn set_loaded(context: u32) {
    LOADED.store(context, Ordering::Relaxed);
}

pub fn kv_requested() -> Option<KvPrecision> {
    KvPrecision::from_code(KV_REQUESTED.load(Ordering::Relaxed))
}

pub fn kv_loaded() -> Option<KvPrecision> {
    KvPrecision::from_code(KV_LOADED.load(Ordering::Relaxed))
}

#[cfg_attr(not(feature = "llm"), allow(dead_code))]
pub(crate) fn set_kv_loaded(kv: Option<KvPrecision>) {
    KV_LOADED.store(kv.map_or(0, KvPrecision::code), Ordering::Relaxed);
}

/// The precision a session built now gets: the runtime setting, else
/// `MAKEPAD_ASSET_AI_LLM_KV`, else q8.
pub fn kv_wanted() -> KvPrecision {
    kv_requested()
        .or_else(|| {
            std::env::var("MAKEPAD_ASSET_AI_LLM_KV")
                .ok()
                .and_then(|v| KvPrecision::parse(&v))
        })
        .unwrap_or(KvPrecision::Q8)
}

pub fn clamp(context: u32, model_max: Option<u32>) -> u32 {
    context.clamp(MIN_CONTEXT, model_max.unwrap_or(u32::MAX).max(MIN_CONTEXT))
}

fn persist(cache_dir: &Path) {
    crate::gpu_yield::clear_pending_hold();
    crate::gpu_yield::reset_relief_state();
    bump_generation();
    crate::control::changed();
    let mut fields = Vec::new();
    if let Some(context) = requested() {
        fields.push(("context", Value::Int(context as i64)));
    }
    if let Some(kv) = kv_requested() {
        fields.push(("kv", json::s(kv.slug())));
    }
    let path = cache_dir.join("llm-context.json");
    if fields.is_empty() {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("clear {}: {e}", path.display());
            }
        }
        return;
    }
    let _ = std::fs::write(
        path,
        format!("{}\n", json::obj(fields).to_json()),
    );
}

/// Set (and persist) the window; returns the value in force after clamping.
pub fn set(cache_dir: &Path, context: u32, model_max: Option<u32>) -> u32 {
    let _guard = SETTINGS.lock().unwrap();
    let context = clamp(context, model_max);
    REQUESTED.store(context, Ordering::Relaxed);
    persist(cache_dir);
    context
}

pub fn clear(cache_dir: &Path) {
    let _guard = SETTINGS.lock().unwrap();
    REQUESTED.store(0, Ordering::Relaxed);
    crate::gpu_yield::set_context_ceiling(None);
    persist(cache_dir);
}

/// Set (and persist) the KV precision.
pub fn set_kv(cache_dir: &Path, kv: KvPrecision) {
    let _guard = SETTINGS.lock().unwrap();
    KV_REQUESTED.store(kv.code(), Ordering::Relaxed);
    persist(cache_dir);
}

/// Read the persisted settings at service start.
pub fn load_persisted(cache_dir: &Path) {
    let Ok(text) = std::fs::read(cache_dir.join("llm-context.json")) else { return };
    let Ok(value) = json::parse(&text) else { return };
    if let Some(n) = value.get("context").and_then(Value::as_u64) {
        REQUESTED.store(clamp(n.min(u32::MAX as u64) as u32, None), Ordering::Relaxed);
    }
    if let Some(kv) = value.get("kv").and_then(Value::as_str).and_then(KvPrecision::parse) {
        KV_REQUESTED.store(kv.code(), Ordering::Relaxed);
    }
}

#[cfg(test)]
pub(crate) fn test_settings() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(all(test, feature = "llm"))]
pub(crate) fn restore_kv_for_test(kv: Option<KvPrecision>) {
    KV_REQUESTED.store(kv.map_or(0, KvPrecision::code), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_sets_clears_and_bumps_generation_without_64k_cap() {
        let _guard = test_settings();
        let dir = std::env::temp_dir().join(format!("llm-context-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let max = Some(262_144);
        let rev = generation();
        assert_eq!(set(&dir, 262_144, max), 262_144);
        assert_eq!(set(&dir, 1 << 30, max), 262_144);
        assert_eq!(set(&dir, 500, max), MIN_CONTEXT);
        clear(&dir);
        assert_eq!(generation(), rev + 4);
        assert_eq!(requested(), None);
        assert!(!dir.join("llm-context.json").exists());
        assert_eq!(set(&dir, 32768, max), 32768);
        set_kv(&dir, KvPrecision::F16);
        REQUESTED.store(0, Ordering::Relaxed);
        KV_REQUESTED.store(0, Ordering::Relaxed);
        load_persisted(&dir);
        assert_eq!(requested(), Some(32768));
        assert_eq!(kv_requested(), Some(KvPrecision::F16));
        assert_eq!(kv_wanted(), KvPrecision::F16);
        set_kv(&dir, KvPrecision::Q8);
        KV_REQUESTED.store(0, Ordering::Relaxed);
        load_persisted(&dir);
        assert_eq!(kv_requested(), Some(KvPrecision::Q8));
        clear(&dir);
        let persisted = std::fs::read(dir.join("llm-context.json")).unwrap();
        assert_eq!(json::parse(&persisted).unwrap(), json::obj(vec![("kv", json::s("q8"))]));
        KV_REQUESTED.store(0, Ordering::Relaxed);
        clear(&dir);
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn kv_names_parse() {
        assert_eq!(KvPrecision::parse("q8_0"), Some(KvPrecision::Q8));
        assert_eq!(KvPrecision::parse(" F16 "), Some(KvPrecision::F16));
        assert_eq!(KvPrecision::parse("q4"), None);
        for kv in KvPrecision::ALL {
            assert_eq!(KvPrecision::parse(kv.slug()), Some(kv));
        }
    }
}
