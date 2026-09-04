use std::cell::RefCell;

#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub detail: String,
    pub loaded: u64,
    pub total: u64,
    pub frac: f32,
}

thread_local! {
    static HOOK: RefCell<Option<Box<dyn FnMut(Progress)>>> = const { RefCell::new(None) };
}

pub fn scope<R>(hook: impl FnMut(Progress) + 'static, f: impl FnOnce() -> R) -> R {
    HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
    let out = f();
    HOOK.with(|slot| {
        *slot.borrow_mut() = None;
    });
    out
}

pub fn emit(progress: Progress) {
    HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(progress);
        }
    });
}

pub fn stage(stage: &str, detail: &str, frac: f32) {
    emit(Progress {
        stage: stage.to_string(),
        detail: detail.to_string(),
        loaded: 0,
        total: 0,
        frac,
    });
}
