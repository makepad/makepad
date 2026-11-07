pub use makepad_widgets;
use makepad_strict_json::{self as json, Value};
use makepad_widgets::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    let Ink = Label{padding: 0 draw_text.color: #dce5ef draw_text.text_style.font_size: 11}
    let Muted = Ink{draw_text.color: #8493a8 draw_text.text_style.font_size: 10}
    let Body = Ink{width: Fill draw_text.wrap: Words}
    let Card = RoundedView{
        height: Fill flow: Down spacing: 7 padding: 16
        draw_bg.color: #17202d draw_bg.border_radius: 10
    }
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Install Makepad"
                window.inner_size: vec2(930, 630)
                pass.clear_color: #0e151f
                body +: {
                    flow: Down spacing: 12 padding: Inset{top: 10 right: 20 bottom: 14 left: 20}
                    View{
                        width: Fill height: Fit flow: Right spacing: 18 align: Align{y: 0.5}
                        RoundedView{
                            width: 42 height: 42 align: Center draw_bg.color: #b8e8d6 draw_bg.border_radius: 11
                            Ink{text: "↗" draw_text.color: #152b25 draw_text.text_style.font_size: 23}
                        }
                        View{width: Fill height: Fit flow: Down spacing: 5
                            Ink{text: "Install Makepad" draw_text.color: #f1f6fb draw_text.text_style.font_size: 22}
                            Muted{text: "Everything on your USB. A fresh start on this computer."}
                        }
                        simulation := CheckBox{text: "Simulation" active: true}
                    }
                    RoundedView{
                        width: Fill height: Fit flow: Right spacing: 12 padding: 8
                        draw_bg.color: #15352f draw_bg.border_radius: 6
                        mode_label := Muted{text: "SIMULATION" draw_text.color: #8fe2bf}
                        mode_description := Body{text: "No drives will be changed." draw_text.color: #c0d9cf draw_text.text_style.font_size: 10}
                    }
                    choose_page := View{
                        width: Fill height: 250 flow: Right spacing: 12
                        Card{
                            width: 300
                            Muted{text: "SOURCE  /  KEEP CONNECTED"}
                            source_name := Body{text: "Installer USB" draw_text.color: #f0f5fc draw_text.text_style.font_size: 14}
                            source_detail := Body{text: "Reading the USB..." draw_text.color: #8493a8 draw_text.text_style.font_size: 10}
                            View{width: Fill height: 1 show_bg: true draw_bg.color: #283341 margin: Inset{top: 5 bottom: 5}}
                            Body{text: "Linux + Makepad WM"}
                            Body{text: "Apps + AI models"}
                            Body{text: "Source Library + full dataset"}
                            Body{text: "AI coding tools"}
                        }
                        Card{
                            width: Fill
                            View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
                                Muted{text: "DESTINATION  /  REPLACE EVERYTHING"}
                                View{width: Fill height: 1}
                                refresh := Button{text: "Refresh" height: 28}
                            }
                            drive := DropDown{width: Fill height: 40 labels: ["Scanning drives..."]}
                            destination_detail := Body{text: "" draw_text.text_style.font_size: 10}
                            destination_reason := Body{text: "" draw_text.color: #f2bd8a draw_text.text_style.font_size: 10}
                            View{width: Fill height: 1 show_bg: true draw_bg.color: #283341 margin: Inset{top: 2 bottom: 3}}
                            Ink{text: "Fast boot cleanup + fresh GRUB" draw_text.color: #b8e8d6}
                            full_wipe := CheckBox{text: "Overwrite every sector instead (slow)"}
                            Muted{text: "Fast cleanup clears boot data and partition structures."}
                        }
                    }
                    review_page := RoundedView{
                        visible: false width: Fill height: Fit flow: Down spacing: 6 padding: 10
                        draw_bg.color: #342624 draw_bg.border_radius: 6
                        warning := Body{text: "All partitions and files on the selected drive will be replaced." draw_text.color: #f5be9f draw_text.text_style.font_size: 10}
                        confirm_group := View{
                            width: Fill height: Fit flow: Down spacing: 4
                            confirmation_hint := Body{text: "" draw_text.text_style.font_size: 10}
                            View{width: Fill height: 36 flow: Right spacing: 10
                                confirmation := TextInput{width: Fill height: Fill empty_text: "Exact erase phrase"}
                                password := TextInput{width: 260 height: Fill empty_text: "Administrator password" is_password: true autocorrect: Disabled autocapitalize: None}
                            }
                            acknowledge := CheckBox{text: "I have backed up this drive and want to erase it."}
                        }
                    }
                    progress_page := View{
                        visible: false width: Fill height: Fit flow: Down spacing: 5
                        View{width: Fill height: Fit flow: Right
                            phase := Body{text: "Preparing..."}
                            percentage := Muted{text: "0%"}
                        }
                        progress := ProgressBar{width: Fill height: 6 value: 0.0}
                        progress_detail := Body{text: "" draw_text.color: #8493a8 draw_text.text_style.font_size: 9}
                    }
                    done_page := View{
                        visible: false width: Fill height: Fit flow: Down spacing: 2
                        done_title := Ink{text: "Finished" draw_text.color: #8fe2bf draw_text.text_style.font_size: 14}
                        done_detail := Body{text: "" draw_text.text_style.font_size: 10}
                    }
                    status := Body{text: "" draw_text.color: #f2bd8a draw_text.text_style.font_size: 10 visible: false}
                    View{width: Fill height: Fill}
                    View{
                        width: Fill height: Fit flow: Right spacing: 12 align: Align{y: 0.5}
                        back := Button{text: "Change drive" visible: false height: 36}
                        View{width: Fill height: Fit flow: Down
                            Ink{text: "Your USB stays intact." draw_text.color: #a5b6ca draw_text.text_style.font_size: 10}
                            Muted{text: "Firmware infections are outside the scope of a disk install." draw_text.text_style.font_size: 8}
                        }
                        primary := ButtonFlat{
                            text: "Review installation" width: 200 height: 42
                            draw_bg +: {color: #b8e8d6 color_hover: #d2f5e7 color_down: #97cdb9 color_focus: #b8e8d6 border_size: 0 border_radius: 7}
                            draw_text +: {color: #122c23 color_hover: #122c23 color_down: #122c23 color_focus: #122c23}
                        }
                    }
                }
            }
        }
    }
}

struct Job {
    operation: &'static str,
    simulation: bool,
    request: String,
    password: String,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn size(value: &Value) -> String {
    format!("{:.1} GB", value.get("size").and_then(Value::as_u64).unwrap_or(0) as f64 / 1e9)
}

fn object_string(value: &Value) -> String {
    let mut out = String::new();
    value.write_into(&mut out);
    out
}

fn helper_path() -> PathBuf {
    let installed = PathBuf::from("/usr/lib/makepad-arch-installer/helper.py");
    if installed.is_file() {
        return installed;
    }
    if let Some(root) = std::env::var_os("MAKEPAD_WM_ROOT") {
        let path = PathBuf::from(root).join("tools/arch_installer/helper.py");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("helper.py")
}

fn perform(mut job: Job, results: &mpsc::Sender<Value>) -> Result<(), String> {
    let mut command = if job.operation == "install" {
        let mut c = Command::new("/usr/bin/sudo");
        c.args(["-S", "-k", "-p", "", "/usr/lib/makepad-arch-installer/helper.py"]);
        c
    } else {
        let mut c = Command::new("python3");
        c.arg("-I").arg(helper_path());
        c
    };
    command.arg(job.operation);
    if job.simulation && job.operation != "simulate" {
        command.arg("--simulation");
    }
    command.args(["--request", &job.request]);
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("Could not start the installer helper: {e}"))?;
    if job.operation == "install" {
        if let Some(mut input) = child.stdin.take() {
            let write_result = writeln!(input, "{}", job.password);
            // Do not retain the password or include it in any progress/error message.
            let mut bytes = std::mem::take(&mut job.password).into_bytes();
            bytes.fill(0);
            write_result.map_err(|_| "Could not send administrator authentication".to_string())?;
        }
    } else {
        drop(child.stdin.take());
    }
    let mut reported_error = false;
    for line in BufReader::new(child.stdout.take().ok_or("No helper output")?).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let value = json::parse(line.as_bytes()).map_err(|_| "Invalid installer helper response")?;
        reported_error |= text(&value, "kind") == "error";
        if results.send(value).is_err() {
            break;
        }
        SignalToUI::set_ui_signal();
    }
    let mut stderr = String::new();
    if let Some(mut error) = child.stderr.take() {
        let _ = error.read_to_string(&mut stderr);
    }
    if !child.wait().map_err(|e| e.to_string())?.success() && !reported_error {
        return Err(if stderr.trim().is_empty() { "The installer stopped before completing.".into() } else { stderr.trim().into() });
    }
    Ok(())
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] commands: Option<SyncSender<Job>>,
    #[rust] results: Option<Receiver<Value>>,
    #[rust] inventory: Option<Value>,
    #[rust] plan: Option<Value>,
    #[rust] selection: usize,
    #[rust] simulation: bool,
    #[rust] locked: bool,
    #[rust] full_wipe: bool,
    #[rust] busy: bool,
    #[rust] page: usize,
    #[rust] message: String,
}

impl App {
    fn selected(&self) -> Option<&Value> {
        self.inventory.as_ref()?.get("destinations")?.as_arr()?.get(self.selection)
    }

    fn request(&self, cx: &mut Cx) -> Value {
        let empty = Value::Null;
        let target = self.selected().unwrap_or(&empty);
        let source = self.inventory.as_ref().and_then(|v| v.get("source")).unwrap_or(&empty);
        json::obj(vec![
            ("destination", json::s(text(target, "path"))),
            ("destination_fingerprint", json::s(text(target, "fingerprint"))),
            ("source_fingerprint", json::s(text(source, "fingerprint"))),
            ("cleanup", json::s(if self.full_wipe { "full" } else { "fast" })),
            ("confirmation", json::s(self.ui.text_input(cx, ids!(confirmation)).text())),
            ("acknowledged", Value::Bool(self.ui.check_box(cx, ids!(acknowledge)).active(cx))),
        ])
    }

    fn submit(&mut self, cx: &mut Cx, operation: &'static str) {
        if self.busy { return; }
        let request = object_string(&self.request(cx));
        let password = if operation == "install" { self.ui.text_input(cx, ids!(password)).text() } else { String::new() };
        self.ui.text_input(cx, ids!(password)).set_text(cx, "");
        let job = Job { operation, simulation: self.simulation, request, password };
        match self.commands.as_ref().map(|tx| tx.try_send(job)) {
            Some(Ok(())) => {
                self.busy = true;
                self.message.clear();
            }
            _ => self.message = "The installer is busy. Please try again.".into(),
        }
        self.render(cx);
    }

    fn render(&mut self, cx: &mut Cx) {
        self.ui.check_box(cx, ids!(simulation)).set_active(cx, self.simulation, Animate::No);
        self.ui.check_box(cx, ids!(full_wipe)).set_active(cx, self.full_wipe, Animate::No);
        self.ui.view(cx, ids!(choose_page)).set_visible(cx, true);
        self.ui.view(cx, ids!(review_page)).set_visible(cx, self.page == 1);
        self.ui.view(cx, ids!(progress_page)).set_visible(cx, self.page == 2);
        self.ui.view(cx, ids!(done_page)).set_visible(cx, self.page == 3);
        self.ui.view(cx, ids!(confirm_group)).set_visible(cx, !self.simulation);
        self.ui.button(cx, ids!(back)).set_visible(cx, self.page == 1);
        self.ui.button(cx, ids!(back)).set_enabled(cx, !self.busy);
        self.ui.button(cx, ids!(refresh)).set_enabled(cx, !self.busy);
        self.ui.widget(cx, ids!(drive)).set_disabled(cx, self.busy);
        self.ui.widget(cx, ids!(simulation)).set_disabled(cx, self.busy || self.locked);
        self.ui.widget(cx, ids!(simulation)).set_visible(cx, !self.locked);
        self.ui.widget(cx, ids!(full_wipe)).set_disabled(cx, self.busy);
        self.ui.label(cx, ids!(status)).set_text(cx, &self.message);
        self.ui.widget(cx, ids!(status)).set_visible(cx, !self.message.is_empty());
        if let Some(inventory) = &self.inventory {
            let source = inventory.get("source").unwrap_or(&Value::Null);
            self.ui.label(cx, ids!(source_name)).set_text(cx, if source.is_null() { "Boot from the installer USB" } else { text(source, "name") });
            let detail = if source.is_null() { text(inventory, "source_error").to_string() } else { format!("{}  ·  {}", size(source), text(source, "path")) };
            self.ui.label(cx, ids!(source_detail)).set_text(cx, &detail);
        }
        if let Some(drive) = self.selected() {
            let detail = format!("{}  ·  {}\nSerial: {}", text(drive, "path"), size(drive), text(drive, "serial"));
            self.ui.label(cx, ids!(destination_detail)).set_text(cx, &detail);
            self.ui.label(cx, ids!(destination_reason)).set_text(cx, text(drive, "reason"));
            self.ui.widget(cx, ids!(destination_reason)).set_visible(cx, !text(drive, "reason").is_empty());
        }
        self.ui.label(cx, ids!(mode_label)).set_text(cx, if self.simulation { "SIMULATION" } else { "REAL INSTALLATION" });
        self.ui.label(cx, ids!(mode_description)).set_text(cx, if self.locked { "Demo locked — real drives cannot be erased." } else if self.simulation { "No drives will be changed." } else { "The selected destination will be erased." });
        self.ui.label(cx, ids!(warning)).set_text(cx, if self.simulation {
            "Ready to simulate: boot cleanup → complete USB copy → GRUB → automatic boot."
        } else { "Every existing partition and file on the selected drive will be replaced. This cannot be undone." });
        let mut ready = self.selected().is_some_and(|v| text(v, "reason").is_empty())
            && self.inventory.as_ref().is_some_and(|v| text(v, "source_error").is_empty());
        if self.page == 1 && !self.simulation {
            ready &= self.plan.as_ref().is_some_and(|p| text(p, "confirmation") == self.ui.text_input(cx, ids!(confirmation)).text())
                && self.ui.check_box(cx, ids!(acknowledge)).active(cx)
                && !self.ui.text_input(cx, ids!(password)).text().is_empty();
        }
        self.ui.button(cx, ids!(primary)).set_text(cx, match self.page {
            0 => "Review installation",
            1 if self.simulation => "Run simulation",
            1 => "Erase drive and install",
            2 => "Installation in progress",
            _ => "Start again",
        });
        self.ui.button(cx, ids!(primary)).set_enabled(cx, !self.busy && self.page != 2 && (ready || self.page == 3));
        self.ui.redraw(cx);
    }

    fn destination_details(&mut self, cx: &mut Cx) {
        self.page = 0;
        self.plan = None;
        self.message.clear();
        self.ui.text_input(cx, ids!(confirmation)).set_text(cx, "");
        self.ui.text_input(cx, ids!(password)).set_text(cx, "");
        self.ui.check_box(cx, ids!(acknowledge)).set_active(cx, false, Animate::No);
        self.render(cx);
    }

    fn receive(&mut self, cx: &mut Cx) {
        let replies: Vec<_> = self.results.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for value in replies {
            match text(&value, "kind") {
                "scan" => {
                    self.busy = false;
                    let labels = value.get("destinations").and_then(Value::as_arr).unwrap_or(&[]).iter()
                        .map(|d| format!("{} — {}{}", text(d, "name"), size(d), if text(d, "reason").is_empty() { "" } else { " (protected / unavailable)" })).collect();
                    self.ui.drop_down(cx, ids!(drive)).set_labels(cx, labels);
                    self.selection = 0;
                    self.ui.drop_down(cx, ids!(drive)).set_selected_item(cx, 0);
                    let source = value.get("source").unwrap_or(&Value::Null);
                    self.ui.label(cx, ids!(source_name)).set_text(cx, if source.is_null() { "Installer USB not booted" } else { text(source, "name") });
                    let detail = if source.is_null() { text(&value, "source_error").to_string() } else { format!("{}  •  {}", size(source), text(source, "path")) };
                    self.ui.label(cx, ids!(source_detail)).set_text(cx, &detail);
                    self.inventory = Some(value);
                    self.destination_details(cx);
                }
                "plan" => {
                    self.busy = false;
                    self.page = 1;
                    self.ui.label(cx, ids!(confirmation_hint)).set_text(cx, &format!("Type {} to confirm this drive", text(&value, "confirmation")));
                    self.plan = Some(value);
                }
                "started" => {
                    self.page = 2;
                    self.ui.progress_bar(cx, ids!(progress)).set_value(cx, 0.0);
                }
                "progress" => {
                    self.page = 2;
                    let percent = value.get("percent").and_then(Value::as_u64).unwrap_or(0).min(100);
                    self.ui.label(cx, ids!(phase)).set_text(cx, text(&value, "phase"));
                    self.ui.label(cx, ids!(progress_detail)).set_text(cx, text(&value, "detail"));
                    self.ui.label(cx, ids!(percentage)).set_text(cx, &format!("{percent}%"));
                    self.ui.progress_bar(cx, ids!(progress)).set_value(cx, percent as f64 / 100.0);
                }
                "complete" => {
                    self.busy = false;
                    self.page = 3;
                    self.ui.label(cx, ids!(done_title)).set_text(cx, if self.simulation { "Simulation complete" } else { "Makepad is installed" });
                    self.ui.label(cx, ids!(done_detail)).set_text(cx, text(&value, "detail"));
                }
                "error" => {
                    self.busy = false;
                    self.page = if self.plan.is_some() { 1 } else { 0 };
                    self.message = text(&value, "detail").into();
                }
                _ => {}
            }
            self.render(cx);
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.simulation = true;
        self.locked = std::env::args().any(|a| a == "--simulate")
            || std::env::var("MAKEPAD_ARCH_INSTALLER_SIMULATE").as_deref() == Ok("1");
        let (tx, jobs) = mpsc::sync_channel::<Job>(1);
        let (results, rx) = mpsc::channel();
        match cx.spawn_worker(move || {
            while let Ok(job) = jobs.recv() {
                if let Err(detail) = perform(job, &results) {
                    let _ = results.send(json::obj(vec![("kind", json::s("error")), ("detail", json::s(detail))]));
                    SignalToUI::set_ui_signal();
                }
            }
        }) {
            Ok(handle) => { handle.detach(); self.commands = Some(tx); self.results = Some(rx); }
            Err(error) => self.message = error.to_string(),
        }
        self.submit(cx, "scan");
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.busy { self.render(cx); return; }
        if let Some(simulation) = self.ui.check_box(cx, ids!(simulation)).changed(actions) {
            self.simulation = simulation || self.locked;
            self.ui.check_box(cx, ids!(simulation)).set_active(cx, self.simulation, Animate::No);
            self.destination_details(cx);
            self.submit(cx, "scan");
        }
        if let Some(full) = self.ui.check_box(cx, ids!(full_wipe)).changed(actions) {
            self.full_wipe = full;
            self.destination_details(cx);
        }
        if let Some(selection) = self.ui.drop_down(cx, ids!(drive)).changed(actions) {
            self.selection = selection;
            self.destination_details(cx);
        }
        if self.ui.button(cx, ids!(refresh)).clicked(actions) { self.submit(cx, "scan"); }
        if self.ui.button(cx, ids!(back)).clicked(actions) {
            self.destination_details(cx);
        }
        if self.ui.button(cx, ids!(primary)).clicked(actions) {
            match self.page {
                0 => self.submit(cx, "plan"),
                1 => self.submit(cx, if self.simulation { "simulate" } else { "install" }),
                3 => { self.page = 0; self.plan = None; self.submit(cx, "scan"); }
                _ => {}
            }
        }
        self.render(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.busy && !self.simulation && matches!(self.page, 1 | 2) {
            if let Event::WindowCloseRequested(close) = event {
                close.accept_close.set(false);
            }
            if let Event::Custom(raw) = event {
                if raw.contains("CloseRequested") { return; }
            }
        }
        self.receive(cx);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
