pub use makepad_widgets;
use makepad_terminal::widget::MpTerm;
use makepad_widgets::*;

use std::collections::HashMap;
use std::path::PathBuf;

use makepad_loader::progress::Progress;
use makepad_loader::{
    default_root, env_bat_path, list_apps, run_install, AppInfo, InstallOpts,
};

app_main!(
    App,
    font_assets: [
        "makepad_widgets/resources/jetbrains_mono_variable.ttf",
        "makepad_widgets/resources/fa-solid-900.ttf",
    ]
);

enum UiMsg {
    Progress(Progress),
    Apps(Vec<AppInfo>),
    Error(String),
    Git(Result<String, String>),
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Logo = Svg{
        width: 240
        height: 35
        animating: false
        draw_svg +: {
            svg: crate_resource("self://resources/logo_makepad.svg")
        }
    }

    let LicenseCard = RoundedView{
        width: Fill
        height: Fit
        padding: 8
        spacing: 2
        flow: Down
        draw_bg +: {
            color: #1a1a22
            border_radius: 6.0
            border_size: 1.0
            border_color: #2a2a33
        }
    }

    mod.widgets.AppListBase = #(AppList::register_widget(vm))
    mod.widgets.AppList = set_type_default() do mod.widgets.AppListBase{
        width: Fill
        height: Fit
        flow: Down
        Tile := RoundedView{
            width: 160
            height: 124
            padding: 10
            spacing: 6
            flow: Down
            draw_bg +: {
                color: #1a1a22
                border_radius: 10.0
                border_size: 1.0
                border_color: #2a2a33
            }
            title := Label{width: Fill text: "" draw_text.color: #f4f4f8}
            pkg := Label{width: Fill text: "" draw_text.color: #888}
            run := Button{width: Fill text: "Run"}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1100, 720)
                window.title: "Makepad"
                pass.clear_color: #111118
                body +: {
                    pages := PageFlip{
                        width: Fill
                        height: Fill
                        active_page: @eula
                        flow: Down

                        eula := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            padding: 16
                            spacing: 8
                            Logo{}
                            Label{
                                width: Fill
                                text: "Accept every license, then Install. Vendors download into a private folder. Nothing is written first."
                                draw_text.color: #aaaab4
                            }
                            LicenseCard{
                                Label{text: "Microsoft Build Tools + Windows SDK" draw_text.color: #f4f4f8}
                                LinkLabel{text: "Windows SDK license" url: "https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license"}
                                eula_ms := CheckBox{text: "I accept Microsoft's terms"}
                            }
                            LicenseCard{
                                Label{text: "NVIDIA CUDA" draw_text.color: #f4f4f8}
                                LinkLabel{text: "NVIDIA CUDA EULA" url: "https://docs.nvidia.com/cuda/eula/"}
                                eula_nv := CheckBox{text: "I accept NVIDIA's EULA"}
                            }
                            LicenseCard{
                                Label{text: "Rust (Apache-2.0 / MIT)" draw_text.color: #f4f4f8}
                                LinkLabel{text: "Rust licenses" url: "https://www.rust-lang.org/policies/licenses"}
                                eula_rust := CheckBox{text: "I accept the Rust notices"}
                            }
                            install := Button{
                                width: Fill
                                text: "Install Makepad"
                                enabled: false
                            }
                            install_hint := Label{
                                width: Fill
                                text: "Accept all three licenses to install"
                                draw_text.color: #888
                            }
                        }

                        work := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            padding: 16
                            spacing: 10
                            Logo{width: 200 height: 29}
                            H2{text: "Installing" draw_text.color: #f4f4f8}
                            stage := Label{text: "Starting…" draw_text.color: #ccc}
                            track := RoundedView{
                                width: Fill
                                height: 12
                                draw_bg +: {color: #2a2a33 border_radius: 6.0}
                                bar := RoundedView{
                                    width: 4
                                    height: Fill
                                    draw_bg +: {color: #3d8bfd border_radius: 6.0}
                                }
                            }
                            detail := Label{
                                width: Fill
                                height: Fill
                                text: ""
                                draw_text.color: #888
                            }
                        }

                        apps := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            padding: 12
                            spacing: 8
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8
                                align: Align{y: 0.5}
                                Logo{width: 190 height: 28}
                                View{width: Fill height: 1}
                                Label{text: "branch" draw_text.color: #888}
                                branch_main := RadioButton{text: "main"}
                                branch_dev := RadioButton{text: "dev"}
                                branch_work := RadioButton{text: "work"}
                                git_pull := Button{text: "Git pull"}
                            }
                            app_list := mod.widgets.AppList{
                                width: Fill
                                height: Fit
                            }
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8
                                align: Align{y: 0.5}
                                compile_label := Label{
                                    width: Fit
                                    text: "compile"
                                    draw_text.color: #888
                                }
                                compile_track := RoundedView{
                                    width: Fill
                                    height: 10
                                    draw_bg +: {color: #2a2a33 border_radius: 5.0}
                                    compile_bar := RoundedView{
                                        width: 4
                                        height: Fill
                                        draw_bg +: {color: #3d8bfd border_radius: 5.0}
                                    }
                                }
                                compile_frac := Label{
                                    width: Fit
                                    text: ""
                                    draw_text.color: #aaa
                                }
                            }
                            RoundedView{
                                width: Fill
                                height: Fill
                                padding: 4
                                draw_bg +: {color: #0d0d12 border_radius: 6.0}
                                term := mod.widgets.MpTerm{
                                    width: Fill
                                    height: Fill{min: 240.}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, Widget)]
pub struct AppList {
    #[deref]
    view: View,
    #[rust]
    apps: Vec<AppInfo>,
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    items: HashMap<usize, WidgetRef>,
}

impl ScriptHook for AppList {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.templates.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if apply.is_eval() {
            return;
        }
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(id) = kv.key.as_id() {
                        if let Some(template_obj) = kv.value.as_object() {
                            self.templates
                                .insert(id, vm.bx.heap.new_object_ref(template_obj));
                        }
                    }
                }
            });
        }
    }
}

impl AppList {
    fn tile(&mut self, cx: &mut Cx, index: usize) -> WidgetRef {
        if let Some(existing) = self.items.get(&index) {
            return existing.clone();
        }
        let Some(template_ref) = self.templates.get(&live_id!(Tile)) else {
            return WidgetRef::empty();
        };
        let template_value: ScriptValue = template_ref.as_object().into();
        let Some(vm_id) = cx.script_ref_vm_id(template_ref) else {
            return WidgetRef::empty();
        };
        let widget = cx.with_script_vm_id(vm_id, |vm| WidgetRef::script_from_value(vm, template_value));
        self.items.insert(index, widget.clone());
        widget
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) -> Option<String> {
        for (index, item) in self.items.iter() {
            if item.button(cx, ids!(run)).clicked(actions) {
                return self.apps.get(*index).map(|a| a.package.clone());
            }
        }
        None
    }
}

impl Widget for AppList {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(
            walk,
            Layout {
                flow: Flow::right_wrap(),
                spacing: 10.0,
                wrap_spacing: 10.0,
                ..Layout::default()
            },
        );
        let n = self.apps.len();
        for index in 0..n {
            let tile = self.tile(cx, index);
            tile.label(cx, ids!(title))
                .set_text(cx, &self.apps[index].title);
            tile.label(cx, ids!(pkg))
                .set_text(cx, &self.apps[index].package);
            tile.draw_walk_all(
                cx,
                &mut Scope::empty(),
                Walk {
                    width: Size::Fixed(160.0),
                    height: Size::Fixed(124.0),
                    ..Walk::default()
                },
            );
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for tile in self.items.values() {
            tile.handle_event(cx, event, scope);
        }
        self.view.handle_event(cx, event, scope);
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    events: ToUIReceiver<UiMsg>,
    #[rust]
    accept_ms: bool,
    #[rust]
    accept_nv: bool,
    #[rust]
    accept_rust: bool,
    #[rust]
    busy: bool,
    #[rust]
    on_apps: bool,
    #[rust]
    branch: String,
    #[rust]
    src_dir: Option<PathBuf>,
    #[rust]
    compile_clock: NextFrame,
}

impl App {
    fn sync_install_enabled(&self, cx: &mut Cx) {
        let on = self.accept_ms && self.accept_nv && self.accept_rust && !self.busy;
        let btn = self.ui.button(cx, ids!(install));
        btn.set_enabled(cx, on);
        self.ui.widget(cx, ids!(install)).set_disabled(cx, !on);
        let hint = if on {
            ""
        } else {
            "Accept all three licenses to install"
        };
        self.ui.label(cx, ids!(install_hint)).set_text(cx, hint);
    }

    fn set_bar(&self, cx: &mut Cx, frac: f32) {
        let track = self.ui.view(cx, ids!(track)).area().rect(cx);
        let w = (track.size.x * frac as f64).clamp(4.0, track.size.x.max(4.0));
        let mut bar = self.ui.view(cx, ids!(bar));
        script_apply_eval!(cx, bar, {
            width: #(w)
        });
    }

    fn open_toolchain_term(&mut self, cx: &mut Cx) {
        let root = default_root();
        let cloned = root.join("src");
        let cwd = if cloned.is_dir() {
            cloned
        } else if let Ok(p) = std::env::var("MAKEPAD_LOADER_SRC") {
            std::path::PathBuf::from(p)
        } else {
            root.clone()
        };
        let bat = env_bat_path(&root);
        let cmd = if cfg!(windows) {
            if bat.is_file() {
                format!("call \"{}\" && cmd /k", bat.display())
            } else {
                "cmd /k".into()
            }
        } else {
            format!("cd '{}' && exec \"$SHELL\" -l", cwd.display())
        };
        if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            term.restart_with(cx, Some(cwd), Some(cmd));
        }
    }

    fn run_pkg(&mut self, cx: &mut Cx, package: &str) {
        if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            let line = format!("cargo run --release -p {package}\r");
            if !term.ai_type_bytes(line.as_bytes()) {
                self.open_toolchain_term(cx);
                if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
                    let _ = term.ai_type_bytes(line.as_bytes());
                }
            }
        }
        self.ui
            .label(cx, ids!(compile_label))
            .set_text(cx, "compile");
        self.compile_clock = cx.new_next_frame();
    }

    fn term_echo(&mut self, cx: &mut Cx, line: &str) {
        if let Some(mut term) = self.ui.widget(cx, ids!(term)).borrow_mut::<MpTerm>() {
            let _ = term.ai_type_bytes(format!("echo {line}\r").as_bytes());
        }
    }

    fn src_dir(&self) -> PathBuf {
        if let Some(src) = &self.src_dir {
            return src.clone();
        }
        let root = default_root();
        let cloned = root.join("src");
        if cloned.is_dir() {
            cloned
        } else if let Ok(p) = std::env::var("MAKEPAD_LOADER_SRC") {
            PathBuf::from(p)
        } else {
            root
        }
    }

    fn pull_git(&mut self, cx: &mut Cx) {
        let branch = self.branch.clone();
        let dest = self.src_dir();
        let url = "https://github.com/makepad/makepad".to_string();
        self.term_echo(cx, &format!("git pull origin {branch}"));
        let sender = self.events.sender();
        match cx.spawn_worker(move || {
            let result = makepad_loader::gitclone::pull_branch(&url, &dest, &branch);
            let _ = sender.send(UiMsg::Git(result));
        }) {
            Ok(handle) => handle.detach(),
            Err(e) => {
                self.term_echo(cx, &format!("git pull could not start: {e}"));
            }
        }
    }

    fn set_compile_bar(&self, cx: &mut Cx, frac: f32) {
        let track = self.ui.view(cx, ids!(compile_track)).area().rect(cx);
        let w = (track.size.x * frac as f64).clamp(4.0, track.size.x.max(4.0));
        let mut bar = self.ui.view(cx, ids!(compile_bar));
        script_apply_eval!(cx, bar, {
            width: #(w)
        });
    }

    fn poll_compile_progress(&mut self, cx: &mut Cx) {
        let rows = self
            .ui
            .widget(cx, ids!(term))
            .borrow::<MpTerm>()
            .and_then(|term| term.ai_screen_rows(Some(24)))
            .map(|(rows, _, _)| rows);
        let Some(rows) = rows else {
            return;
        };
        if let Some((pos, len, name)) = parse_cargo_progress(&rows) {
            let frac = if len == 0 {
                0.0
            } else {
                (pos as f32 / len as f32).clamp(0.0, 1.0)
            };
            self.set_compile_bar(cx, frac);
            self.ui
                .label(cx, ids!(compile_frac))
                .set_text(cx, &format!("{pos}/{len}"));
            self.ui
                .label(cx, ids!(compile_label))
                .set_text(cx, &name);
        }
    }
}

fn parse_cargo_progress(rows: &[String]) -> Option<(u32, u32, String)> {
    let mut last = None;
    for row in rows {
        let t = row.trim();
        if t.contains("Finished") {
            if let Some((_, len, _)) = last {
                last = Some((len, len, "finished".into()));
            }
            continue;
        }
        if t.contains("Running `") {
            if let Some((_, len, _)) = last {
                last = Some((len, len, "running".into()));
            }
            continue;
        }
        let building = t.contains("Building") || t.contains("Downloading") || t.contains("Compiling");
        if !building {
            continue;
        }
        if let Some((pos, len, name)) = parse_nm(t) {
            last = Some((pos, len, name));
        }
    }
    last
}

fn parse_nm(t: &str) -> Option<(u32, u32, String)> {
    let slash = t.rfind('/')?;
    let (left, right) = t.split_at(slash);
    let pos: u32 = left
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    let rest = &right[1..];
    let len_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let len: u32 = len_str.parse().ok()?;
    if len == 0 {
        return None;
    }
    let name = rest
        .split(':')
        .nth(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "compile".into());
    Some((pos, len, name))
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        if self.branch.is_empty() {
            self.branch = "work".into();
        }
        self.sync_install_enabled(cx);
        self.ui
            .radio_button(cx, ids!(branch_work))
            .set_active(cx, true, Animate::No);
    }

    fn handle_next_frame(&mut self, cx: &mut Cx, _e: &NextFrameEvent) {
        if self.on_apps {
            self.poll_compile_progress(cx);
            self.compile_clock = cx.new_next_frame();
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(v) = self.ui.check_box(cx, ids!(eula_ms)).changed(actions) {
            self.accept_ms = v;
            self.sync_install_enabled(cx);
        }
        if let Some(v) = self.ui.check_box(cx, ids!(eula_nv)).changed(actions) {
            self.accept_nv = v;
            self.sync_install_enabled(cx);
        }
        if let Some(v) = self.ui.check_box(cx, ids!(eula_rust)).changed(actions) {
            self.accept_rust = v;
            self.sync_install_enabled(cx);
        }
        if let Some(index) = self
            .ui
            .radio_button_set(cx, ids_list!(branch_main, branch_dev, branch_work))
            .selected(cx, actions)
        {
            self.branch = match index {
                0 => "main".into(),
                1 => "dev".into(),
                _ => "work".into(),
            };
        }
        if self.ui.button(cx, ids!(git_pull)).clicked(actions) {
            self.pull_git(cx);
        }
        if self.ui.button(cx, ids!(install)).clicked(actions) {
            if self.busy || !(self.accept_ms && self.accept_nv && self.accept_rust) {
                return;
            }
            self.busy = true;
            self.sync_install_enabled(cx);
            self.ui
                .page_flip(cx, ids!(pages))
                .set_active_page(cx, live_id!(work));
            let sender = self.events.sender();
            let branch = self.branch.clone();
            match cx.spawn_worker(move || {
                let src_env = std::env::var("MAKEPAD_LOADER_SRC").ok().map(PathBuf::from);
                let skip_git = src_env.is_some() || cfg!(not(windows));
                let opts = InstallOpts {
                    skip_cuda: false,
                    skip_git,
                    skip_build: true,
                    src: src_env.or_else(|| {
                        if cfg!(windows) {
                            None
                        } else {
                            std::env::current_dir().ok()
                        }
                    }),
                    branch: Some(branch),
                    ..InstallOpts::default()
                };
                let send = sender.clone();
                let result = run_install(&opts, move |p| {
                    let _ = send.send(UiMsg::Progress(p));
                });
                match result {
                    Ok(apps) => {
                        let _ = sender.send(UiMsg::Apps(apps));
                    }
                    Err(e) => {
                        let _ = sender.send(UiMsg::Error(e));
                    }
                }
            }) {
                Ok(handle) => handle.detach(),
                Err(e) => {
                    self.busy = false;
                    self.ui
                        .label(cx, ids!(detail))
                        .set_text(cx, &format!("could not start installer: {e}"));
                }
            }
        }
        let run = self
            .ui
            .widget(cx, ids!(app_list))
            .borrow_mut::<AppList>()
            .and_then(|mut list| list.handle_actions(cx, actions));
        if let Some(pkg) = run {
            self.run_pkg(cx, &pkg);
        }
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        let mut apps = None;
        let mut err = None;
        while let Ok(msg) = self.events.try_recv() {
            match msg {
                UiMsg::Progress(p) => {
                    self.ui.label(cx, ids!(stage)).set_text(cx, &p.stage);
                    self.ui.label(cx, ids!(detail)).set_text(cx, &p.detail);
                    self.set_bar(cx, p.frac.clamp(0.0, 1.0));
                }
                UiMsg::Apps(list) => apps = Some(list),
                UiMsg::Error(e) => err = Some(e),
                UiMsg::Git(Ok(s)) => {
                    self.term_echo(cx, &s);
                    let src = self.src_dir();
                    if let Ok(list) = list_apps(&src) {
                        if let Some(mut al) =
                            self.ui.widget(cx, ids!(app_list)).borrow_mut::<AppList>()
                        {
                            al.apps = list;
                            al.items.clear();
                        }
                    }
                }
                UiMsg::Git(Err(e)) => {
                    self.term_echo(cx, &format!("git pull failed: {e}"));
                }
            }
        }
        if let Some(e) = err {
            self.busy = false;
            self.ui.label(cx, ids!(detail)).set_text(cx, &e);
        }
        if let Some(list) = apps {
            self.busy = false;
            self.on_apps = true;
            self.src_dir = Some(self.src_dir());
            if let Some(mut al) = self.ui.widget(cx, ids!(app_list)).borrow_mut::<AppList>() {
                al.apps = list;
            }
            self.ui
                .page_flip(cx, ids!(pages))
                .set_active_page(cx, live_id!(apps));
            self.open_toolchain_term(cx);
            self.compile_clock = cx.new_next_frame();
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_terminal::widget::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
