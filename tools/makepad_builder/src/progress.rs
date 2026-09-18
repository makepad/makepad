use std::cell::RefCell;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Unit {
    #[default]
    None,
    Bytes,
    Files,
    Blocks,
    Objects,
}
#[derive(Clone, Debug, Default)]
pub struct Package {
    pub group: String,
    pub name: String,
    pub index: usize,
    pub count: usize,
}
#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub detail: String,
    pub loaded: u64,
    pub total: u64,
    pub frac: f32,
    pub unit: Unit,
    pub package: Package,
}
thread_local! {
    static HOOK: RefCell<Option<Box<dyn FnMut(Progress)>>> = const { RefCell::new(None) };
    static PACKAGE: RefCell<Package> = RefCell::new(Package::default());
}

pub fn scope<R>(hook: impl FnMut(Progress) + 'static, f: impl FnOnce() -> R) -> R {
    let previous = HOOK.with(|slot| slot.replace(Some(Box::new(hook))));
    struct Restore(Option<Box<dyn FnMut(Progress)>>, Package);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOOK.with(|h| {
                h.replace(self.0.take());
            });
            PACKAGE.with(|p| {
                p.replace(std::mem::take(&mut self.1));
            });
        }
    }
    let _restore = Restore(previous, PACKAGE.with(|p| p.replace(Package::default())));
    f()
}
pub fn emit(mut progress: Progress) {
    progress.package = PACKAGE.with(|p| p.borrow().clone());
    HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(progress);
        }
    });
}
pub fn package(group: &str, name: &str, index: usize, count: usize) {
    PACKAGE.with(|p| {
        p.replace(Package {
            group: group.into(),
            name: name.into(),
            index,
            count,
        });
    });
    stage("Preparing", name, 0.0);
}
pub fn stage(stage: &str, detail: &str, frac: f32) {
    emit(Progress {
        stage: stage.into(),
        detail: detail.into(),
        loaded: 0,
        total: 0,
        frac,
        unit: Unit::None,
        package: Package::default(),
    });
}
pub fn measured(stage: &str, detail: &str, loaded: u64, total: u64, unit: Unit) {
    emit(Progress {
        stage: stage.into(),
        detail: detail.into(),
        loaded,
        total,
        frac: if total > 0 {
            loaded as f32 / total as f32
        } else {
            0.0
        },
        unit,
        package: Package::default(),
    });
}
pub fn active() -> bool { HOOK.with(|h| h.borrow().is_some()) }
pub fn message(text: &str) {
    if HOOK.with(|h| h.borrow().is_some()) {
        stage("Working", text, 0.0);
    } else {
        println!("{text}");
    }
}
#[macro_export]
macro_rules! setup_note { ($($args:tt)*) => { $crate::progress::message(&format!($($args)*)) }; }
