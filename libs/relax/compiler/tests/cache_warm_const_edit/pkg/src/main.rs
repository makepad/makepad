// Prints "out tick <n> <probe>=<value> .." every 20 ms; tests/cache_warm_const_edit/run.py edits the
// values while it runs. WARM_EDIT_ONCE=1: one line, then exit; =fill: past the startup window it
// first calls the late readers, so their code is stored but not part of the startup image.
// =many: the startup has slots no other run has, and stays past the mid-run write: this text gets a
// startup manifest of its own.
// Otherwise the late readers run once the file WARM_EDIT_LATE names exists: their stored code is
// first linked after the edit.
mod fields;
mod readers;
mod values;
use std::hint::black_box;

const CONST_MACRO_SAME: u64 = 1;

// Only a macro's arguments name these consts.
#[inline(never)]
fn const_macro() -> u64 {
    format!("{}", values::CONST_MACRO).parse().unwrap()
}

#[inline(never)]
fn const_macro_same() -> u64 {
    println!("macro {}", CONST_MACRO_SAME);
    CONST_MACRO_SAME
}

#[inline(never)]
fn const_direct() -> u64 {
    values::CONST_DIRECT
}

#[inline(never)]
fn const_reader() -> u64 {
    values::CONST_THROUGH
}

#[inline(never)]
fn const_through() -> u64 {
    const_reader()
}

#[inline]
fn small() -> u64 {
    values::CONST_INLINED
}

#[inline(never)]
fn const_inlined() -> u64 {
    small()
}

#[inline(never)]
fn const_derived() -> u64 {
    values::CONST_DERIVED
}

#[inline(never)]
fn const_assoc<T: values::Kind>() -> u64 {
    T::K
}

#[inline(never)]
fn const_assoc_named() -> u64 {
    <values::B as values::Kind>::K
}

#[inline(never)]
fn type_assoc<T: values::Shape>() -> u64 {
    std::mem::size_of::<T::Out>() as u64
}

#[inline(never)]
fn size_arg<T>() -> u64 {
    std::mem::size_of::<T>() as u64
}

#[inline(never)]
fn read_blank<T: values::Blank>() -> u64 {
    T::BL
}

#[inline(never)]
fn read_dflt<T: values::Dflt>() -> u64 {
    T::D
}

#[inline(never)]
fn read_method<T: values::Dflt>() -> u64 {
    T::m()
}

#[inline(never)]
fn assoc_closure<T: values::Kind>() -> u64 {
    let f = || T::K;
    black_box(f)()
}

#[inline(never)]
fn by_value<T: values::Kind>(_: T) -> u64 {
    T::K
}

#[inline(never)]
fn const_opaque() -> u64 {
    by_value(values::make_op())
}

#[inline]
fn small_value<T: values::Kind>(_: T) -> u64 {
    T::K
}

#[inline(never)]
fn const_inferred() -> u64 {
    small_value(values::make_inf())
}

#[inline(never)]
fn by_n<const N: usize>() -> u64 {
    N as u64
}

#[inline(never)]
fn const_generic() -> u64 {
    by_n::<{ values::CONST_GEN }>()
}

#[inline(never)]
fn needs_drop_of<T>() -> u64 {
    std::mem::needs_drop::<T>() as u64
}

#[inline(never)]
fn glue_of<T: Default>() -> u64 {
    drop(T::default());
    values::GLUE_DROPS.load(std::sync::atomic::Ordering::Relaxed)
}

#[inline(never)]
fn impl_added_spec() -> u64 {
    [values::Sp(1)].to_vec()[0].0
}

#[inline(never)]
fn const_pattern() -> u64 {
    match black_box(1u64) {
        values::CONST_PATTERN => 1,
        _ => 2,
    }
}

#[inline(never)]
fn const_len() -> u64 {
    let a = [7u8; values::CONST_LEN];
    black_box(&a);
    a.len() as u64
}

#[inline(never)]
fn const_dep() -> u64 {
    warm_const_dep::DEP_VALUE
}

#[inline(never)]
fn const_include() -> u64 {
    values::CONST_INCLUDE.len() as u64
}

#[inline(never)]
fn include_macro() -> u64 {
    values::DOCS.iter().map(|d| d.text.len()).sum::<usize>() as u64
}

#[inline(never)]
fn include_found() -> u64 {
    values::DOCS.iter().find(|d| d.key == "doc_b").map_or(0, |d| d.text.len() as u64)
}

// found by key through a fn pointer the program stored at its start (Fab's demo builder)
static FIND: std::sync::OnceLock<fn(&str) -> u64> = std::sync::OnceLock::new();
#[inline(never)]
fn include_added() -> u64 {
    let find = FIND.get_or_init(|| |key| values::DOCS.iter().find(|d| d.key == key).map_or(0, |d| d.text.len() as u64));
    find("doc_c")
}

#[inline(never)]
fn include_env() -> u64 {
    values::DOC_E.len() as u64
}

#[inline(never)]
fn body_include() -> u64 {
    include_str!("body.txt").len() as u64
}

#[inline(never)]
fn static_direct() -> u64 {
    values::STATIC_DIRECT
}

#[inline(never)]
fn static_reader() -> u64 {
    values::STATIC_THROUGH
}

#[inline(never)]
fn static_through() -> u64 {
    static_reader()
}

#[inline(never)]
fn body_inlined() -> u64 {
    values::tiny()
}

#[inline(never)]
fn body_late() -> u64 {
    values::late_tiny()
}

#[inline(never)]
fn always_late() -> u64 {
    values::late_always()
}

#[inline(never)]
fn const_late() -> u64 {
    values::CONST_LATE
}

#[inline(never)]
fn static_late() -> u64 {
    values::STATIC_LATE
}

#[inline(never)]
fn every_direct() -> u64 {
    warm_const_dep::EVERY
}

#[inline(never)]
fn every_fn() -> u64 {
    warm_const_dep::every_fn()
}

#[inline(never)]
fn every_inlined() -> u64 {
    warm_const_dep::every_small()
}

#[inline(never)]
fn every_generic() -> u64 {
    warm_const_dep::every_generic::<u32>()
}

#[inline(never)]
fn every_closure() -> u64 {
    let f = || warm_const_dep::EVERY;
    black_box(f)()
}

#[inline(never)]
fn every_assoc() -> u64 {
    <warm_const_dep::EveryT as warm_const_dep::EveryK>::E
}

#[inline(never)]
fn every_derived() -> u64 {
    warm_const_dep::EVERY_DERIVED
}

#[inline(never)]
fn const_nonliteral() -> u64 {
    values::CONST_NONLIT
}

#[inline(never)]
fn const_discr() -> u64 {
    values::Dsc::One as isize as u64
}

#[inline(never)]
fn static_from_const() -> u64 {
    *black_box(&values::STATIC_FROM_CONST)
}

#[inline(never)]
fn static_through_fn() -> u64 {
    *black_box(&values::STATIC_THROUGH_FN)
}

extern "C" {
    fn relax_first_present();
}

#[inline(never)]
fn many_one<const N: usize>() -> u64 {
    black_box(N as u64)
}

macro_rules! many {
    ($($n:literal)*) => { 0 $(+ many_one::<{ $n }>() + many_one::<{ 100 + $n }>() + many_one::<{ 200 + $n }>() + many_one::<{ 300 + $n }>())* };
}

#[inline(never)]
fn many() -> u64 {
    many!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49)
}

// `--features window`: the same program as an app with a window (run.py --window): its restarts
// keep the process and the window.
#[cfg(feature = "window")]
mod window {
    use makepad_widgets::*;

    script_mod! {
        use mod.prelude.widgets.*
        startup() do #(App::script_component(vm)){
            ui: Root{
                main_window := Window{
                    window.inner_size: vec2(320, 200)
                    body +: { Label{text: "const edits"} }
                }
            }
        }
    }

    #[derive(Script, ScriptHook)]
    pub struct App {
        #[live]
        ui: WidgetRef,
    }

    impl AppMain for App {
        fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
            makepad_widgets::script_mod(vm);
            self::script_mod(vm)
        }
        fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
            if matches!(event, Event::Startup) {
                std::thread::spawn(super::run);
            }
            self.ui.handle_event(cx, event, &mut Scope::empty());
        }
    }
    app_main!(App);
    pub fn main() {
        app_main();
    }
}

fn main() {
    #[cfg(feature = "window")]
    window::main();
    #[cfg(not(feature = "window"))]
    run();
}

fn run() {
    let once = std::env::var("WARM_EDIT_ONCE").ok();
    if once.as_deref() == Some("many") {
        black_box(many());
        // what a window's first present tells the JIT: the mid-run write then makes the manifest
        unsafe { relax_first_present() };
        std::thread::sleep(std::time::Duration::from_millis(5500));
    }
    let late_file = std::env::var("WARM_EDIT_LATE").unwrap_or_default();
    std::thread::spawn(fields::blocked_reader);
    std::thread::spawn(fields::short_walk_reader);
    // (the rows read what those threads stored)
    while fields::BLOCKED_READ.load(std::sync::atomic::Ordering::Relaxed) == 0 || fields::SHORT_WALK_READ.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        std::thread::yield_now();
    }
    let mut n: u64 = 0;
    loop {
        if once.as_deref() == Some("fill") {
            println!("out start");
            std::thread::sleep(std::time::Duration::from_millis(5500));
        }
        let late = once.as_deref() == Some("fill") || (!late_file.is_empty() && std::path::Path::new(&late_file).exists());
        let every_late = if late { warm_const_dep::every_late() } else { 0 };
        let (const_late, static_late, body_late, always_late) = if late { (const_late(), static_late(), body_late(), always_late()) } else { (0, 0, 0, 0) };
        let running = fields::Running::new();
        let held_copy = values::cpf_new();
        let field_running_frame = running.u[0] + running.u.len() as u64 * 10;
        let row = [
            ("field_running_frame", field_running_frame),
            ("field_blocked_frame", fields::BLOCKED_READ.load(std::sync::atomic::Ordering::Relaxed)),
            ("field_short_walk", fields::SHORT_WALK_READ.load(std::sync::atomic::Ordering::Relaxed)),
            ("const_direct", const_direct()),
            ("const_through", const_through()),
            ("const_inlined", const_inlined()),
            ("const_derived", const_derived()),
            ("const_assoc", const_assoc::<values::A>()),
            ("const_assoc_named", const_assoc_named()),
            ("const_assoc_foreign", const_assoc::<warm_const_dep::DepT>()),
            ("const_assoc_dep_generic", warm_const_dep::read_dk::<values::E1>()),
            ("const_assoc_wrapped", const_assoc::<Vec<values::W3>>()),
            ("const_assoc_prim", const_assoc::<u32>()),
            ("const_assoc_blanket", read_blank::<values::M1>()),
            ("const_assoc_closure", assoc_closure::<values::Cl>()),
            ("const_assoc_chain", const_assoc::<values::Wrap<values::A4>>()),
            ("const_opaque", const_opaque()),
            ("const_inferred", const_inferred()),
            ("trait_default", read_dflt::<values::Df>()),
            ("method_default", read_method::<values::Df>()),
            ("type_assoc", type_assoc::<values::C>()),
            ("type_assoc_field", size_arg::<values::Holder<values::C2>>()),
            ("const_generic", const_generic()),
            ("const_generic_assoc", const_assoc::<values::G<{ values::CONST_GEN_TY }>>()),
            ("const_array_arg", size_arg::<values::Buf>()),
            ("const_array_assoc", size_arg::<values::BufK>()),
            ("const_array_ty", size_arg::<[u8; values::CONST_ARR_TY]>()),
            ("impl_added", needs_drop_of::<values::Plain>()),
            ("impl_removed", needs_drop_of::<values::Droppy>()),
            ("impl_added_glue", glue_of::<values::Glue>()),
            ("impl_added_spec", impl_added_spec()),
            ("const_macro", const_macro()),
            ("const_macro_same", const_macro_same()),
            ("const_pattern", const_pattern()),
            ("const_len", const_len()),
            ("const_dep", const_dep()),
            ("const_same", values::same_file()),
            ("const_include", const_include()),
            ("body_include", body_include()),
            ("include_macro", include_macro()),
            ("include_found", include_found()),
            ("include_added", include_added()),
            ("include_env", include_env()),
            ("static_direct", static_direct()),
            ("static_through", static_through()),
            ("body_inlined", body_inlined()),
            ("const_nonliteral", const_nonliteral()),
            ("const_discr", const_discr()),
            ("const_array_derived", size_arg::<[u8; values::CONST_ARR_DER]>()),
            ("static_from_const", static_from_const()),
            ("static_through_fn", static_through_fn()),
            ("const_other_new", values::ForNew::new().0 + values::STATIC_OTHER_NEW.0 - 7),
            ("static_through_new", values::STATIC_THROUGH_NEW.0),
            ("const_array_fn", size_arg::<values::BufFn>()),
            ("const_array_macro", size_arg::<values::BufMac>()),
            ("const_array_alias", size_arg::<values::BufAl>()),
            ("const_dep_array", size_arg::<warm_const_dep::DepBuf>()),
            ("const_dep_generic", warm_const_dep::dep_generic()),
            ("const_dep_generic_field", size_arg::<warm_const_dep::DepHolder>()),
            ("const_dep_generic_alias", size_arg::<warm_const_dep::DepAl>()),
            ("const_dep_in_main_type", size_arg::<[u8; warm_const_dep::DEP_FOR_MAIN]>()),
            ("every_direct", every_direct()),
            ("every_fn", every_fn()),
            ("every_inlined", every_inlined()),
            ("every_generic", every_generic()),
            ("every_closure", every_closure()),
            ("every_assoc", every_assoc()),
            ("every_derived", every_derived()),
            ("every_late", every_late),
            ("const_late", const_late),
            ("static_late", static_late),
            ("body_late", body_late),
            ("always_late", always_late),
            ("dyn_method_add", values::dm_held()),
            ("dyn_method_add_late", if late { values::dm_late() } else { 0 }),
            ("dyn_vtable_fresh", values::dv_read()),
            ("dyn_impl_removed", values::ri_read()),
            ("enum_insert", values::ev_read()),
            ("enum_insert_cast", values::lv_read()),
            ("enum_insert_hash", values::hk_read()),
            ("field_copy", values::cp_read()),
            ("field_copy_frame", held_copy.a as u64 + held_copy.b),
        ];
        let mut line = format!("out tick {n}");
        for (name, v) in row.into_iter().chain(readers::row(late)) {
            line += &format!(" {name}={v}");
        }
        println!("{line}");
        if once.is_some() {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // (this is a thread beside the window here)
    #[cfg(feature = "window")]
    std::process::exit(0);
}
