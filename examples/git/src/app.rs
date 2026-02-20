use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use makepad_git::{
    apply_pack_and_checkout, build_info_refs_request, build_ls_refs_head_request,
    build_upload_pack_request, extract_pack_from_response, parse_info_refs_response,
    parse_ls_refs_head_response, GitHttpMethod, GitHttpRequest, GitHttpResponse, GitError,
    HttpSyncHooks, ObjectId, RemoteHead, Repository,
};
use makepad_widgets::*;

const REMOTE_URL: &str = "https://github.com/makepad/makepad";
const DEST_PATH: &str = "local/testcheckout";
const POLL_INTERVAL_SECS: f64 = 30.0;
const MAX_EVENT_LOGS: usize = 20000;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let GitLogList = #(GitLogList::register_widget(vm)) {
        width: Fill
        height: Fill
        
        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true
            selectable: true

            Line := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 1 bottom: 1 left: 0 right: 0}
                padding: Inset{top: 2 bottom: 2 left: 8 right: 8}
                draw_bg.color: #x151e34
                draw_bg.border_radius: 4.0

                line_text := Label {
                    width: Fill
                    text: ""
                    draw_text.color: #xb9c5eb
                    draw_text.text_style: theme.font_regular{font_size: 9}
                }
            }

            Empty := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 1 bottom: 1 left: 0 right: 0}
                padding: Inset{top: 2 bottom: 2 left: 8 right: 8}
                draw_bg.color: #x10182a
                draw_bg.border_radius: 4.0
                empty_text := Label {
                    text: "No events yet"
                    draw_text.color: #x6677a0
                    draw_text.text_style.font_size: 9
                }
            }
        }
    }

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900 720)
                pass.clear_color: vec4(0.06, 0.08, 0.12, 1.0)
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 12
                    padding: Inset{top: 22 bottom: 20 left: 22 right: 22}

                    RoundedView{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 5
                        padding: Inset{top: 16 bottom: 16 left: 16 right: 16}
                        draw_bg.color: #x16223c
                        draw_bg.border_radius: 12.0

                        Label{
                            text: "Git HTTP Checkout Console"
                            draw_text.color: #xfff
                            draw_text.text_style: theme.font_bold{font_size: 20}
                        }
                        Label{
                            text: "Polls remote HEAD and runs checkout only when you click Checkout (HTTP)."
                            draw_text.color: #x9aabd4
                            draw_text.text_style.font_size: 10
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10

                        RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 4
                            padding: Inset{top: 10 bottom: 10 left: 12 right: 12}
                            draw_bg.color: #x121a2e
                            draw_bg.border_radius: 10.0

                            Label{
                                text: "Remote"
                                draw_text.color: #x7f93c0
                                draw_text.text_style.font_size: 9
                            }
                            remote_value := Label{
                                text: ""
                                draw_text.color: #xdeebff
                                draw_text.text_style: theme.font_regular{font_size: 11}
                            }
                        }

                        RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 4
                            padding: Inset{top: 10 bottom: 10 left: 12 right: 12}
                            draw_bg.color: #x121a2e
                            draw_bg.border_radius: 10.0

                            Label{
                                text: "Destination"
                                draw_text.color: #x7f93c0
                                draw_text.text_style.font_size: 9
                            }
                            dest_value := Label{
                                text: ""
                                draw_text.color: #xdeebff
                                draw_text.text_style: theme.font_regular{font_size: 11}
                            }
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10

                        poll_now := Button{
                            width: Fit
                            height: 36
                            text: "Poll Now"
                            draw_bg +: {
                                color: uniform(#x355fd1)
                                color_hover: uniform(#x4875ef)
                                color_down: uniform(#x284eb5)
                                border_radius: 8.0
                            }
                            draw_text +: {
                                color: #xfff
                                text_style +: {font_size: 11}
                            }
                        }

                        checkout_http := Button{
                            width: Fit
                            height: 36
                            text: "Checkout (HTTP)"
                            draw_bg +: {
                                color: uniform(#x2f4f7f)
                                color_hover: uniform(#x3c66a4)
                                color_down: uniform(#x263f66)
                                border_radius: 8.0
                            }
                            draw_text +: {
                                color: #xf2f6ff
                                text_style +: {font_size: 11}
                            }
                        }
                    }

                    RoundedView{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 6
                        padding: Inset{top: 12 bottom: 12 left: 12 right: 12}
                        draw_bg.color: #x121a2f
                        draw_bg.border_radius: 10.0

                        status_label := Label{
                            text: "Idle"
                            draw_text.color: #xfff
                            draw_text.text_style: theme.font_bold{font_size: 12}
                        }

                        detail_label := Label{
                            text: "Waiting for first poll"
                            draw_text.color: #xa0b2de
                            draw_text.text_style.font_size: 10
                        }
                    }

                    RoundedView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 8
                        padding: Inset{top: 10 bottom: 10 left: 10 right: 10}
                        draw_bg.color: #x0d1424
                        draw_bg.border_radius: 12.0

                        Label{
                            text: "Log View"
                            draw_text.color: #xc5d5ff
                            draw_text.text_style: theme.font_bold{font_size: 11}
                        }

                        log_list := GitLogList{}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum SyncPhase {
    #[default]
    Idle,
    AwaitLsRefs,
    AwaitInfoRefs,
    AwaitUploadPack,
}

static EVENT_LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn event_logs() -> &'static Mutex<Vec<String>> {
    EVENT_LOGS.get_or_init(|| Mutex::new(Vec::new()))
}

fn event_log_snapshot() -> Vec<String> {
    event_logs().lock().map(|logs| logs.clone()).unwrap_or_default()
}

fn push_event_log(line: String) {
    if let Ok(mut logs) = event_logs().lock() {
        logs.push(line);
        if logs.len() > MAX_EVENT_LOGS {
            let drop_n = logs.len() - MAX_EVENT_LOGS;
            logs.drain(0..drop_n);
        }
    }
}

struct CheckoutLogHooks {
    files: Vec<String>,
}

impl HttpSyncHooks for CheckoutLogHooks {
    fn on_checkout_file(&mut self, path: &str) {
        self.files.push(path.to_string());
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct GitLogList {
    #[deref]
    view: View,
}

impl Widget for GitLogList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let lines = event_log_snapshot();
        let line_count = lines.len();
        let range_end = line_count.saturating_sub(1);

        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, range_end);
                while let Some(item_id) = list.next_visible_item(cx) {
                    if line_count == 0 {
                        let item = list.item(cx, item_id, id!(Empty));
                        item.draw_all_unscoped(cx);
                    } else {
                        let item = list.item(cx, item_id, id!(Line));
                        if let Some(line) = lines.get(item_id) {
                            item.label(cx, ids!(line_text)).set_text(cx, line);
                        } else {
                            item.label(cx, ids!(line_text)).set_text(cx, "");
                        }
                        item.draw_all_unscoped(cx);
                    }
                }
            }
        }

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn set_detail(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(detail_label)).set_text(cx, text);
    }

    fn log_event(&mut self, cx: &mut Cx, text: impl Into<String>) {
        self.log_counter += 1;
        let line = format!("#{:04} {}", self.log_counter, text.into());
        push_event_log(line);
        self.ui.widget(cx, ids!(log_list)).redraw(cx);
    }

    fn debug_portal_list_state(&mut self, cx: &mut Cx, reason: &str) {
        let state = self
            .ui
            .portal_list(cx, ids!(log_list.list))
            .debug_scroll_state_line();
        if state != self.last_portal_scroll_state {
            self.last_portal_scroll_state = state.clone();
            log!("PortalList[{}] {}", reason, state);
        }
    }

    fn destination_display_path() -> String {
        let rel = PathBuf::from(DEST_PATH);
        if let Ok(cwd) = std::env::current_dir() {
            cwd.join(rel).display().to_string()
        } else {
            DEST_PATH.to_string()
        }
    }

    fn local_head_oid(&self) -> Option<ObjectId> {
        Repository::open(Path::new(DEST_PATH))
            .ok()
            .and_then(|repo| repo.head_oid().ok())
    }

    fn short_oid(oid: &ObjectId) -> String {
        let hex = oid.to_hex();
        hex.chars().take(8).collect()
    }

    fn poll_remote(&mut self, cx: &mut Cx) {
        if self.active_request_id.is_some() {
            return;
        }

        let request = match build_ls_refs_head_request(REMOTE_URL) {
            Ok(req) => req,
            Err(err) => {
                self.fail(cx, err.to_string());
                return;
            }
        };

        self.phase = SyncPhase::AwaitLsRefs;
        self.send_git_request(cx, request);
        self.set_status(cx, "Polling remote HEAD hash...");
        self.log_event(cx, "Poll: requesting remote HEAD hash (protocol v2 ls-refs)");
    }

    fn checkout_http(&mut self, cx: &mut Cx) {
        if self.active_request_id.is_some() {
            self.set_status(cx, "Request already in flight");
            self.log_event(cx, "Checkout ignored: request already in flight");
            return;
        }

        self.force_full_checkout = true;
        self.pending_remote_head = None;
        self.phase = SyncPhase::AwaitInfoRefs;
        self.send_git_request(cx, build_info_refs_request(REMOTE_URL, None));
        self.set_status(cx, "Checkout: requesting refs over HTTP...");
        self.set_detail(cx, "Preparing full depth=1 checkout from remote HEAD.");
        self.log_event(cx, "Checkout: started full HTTP depth=1 checkout");
        self.log_event(cx, "Checkout: requesting info/refs over HTTP");
    }

    fn send_git_request(&mut self, cx: &mut Cx, req: GitHttpRequest) {
        self.request_counter += 1;
        let request_id = LiveId::from_str_num("git_https_sync", self.request_counter);
        self.active_request_id = Some(request_id);

        let method = match req.method {
            GitHttpMethod::Get => HttpMethod::GET,
            GitHttpMethod::Post => HttpMethod::POST,
        };

        let mut http_req = HttpRequest::new(req.url, method);
        for (name, value) in req.headers {
            http_req.set_header(name, value);
        }
        if !req.body.is_empty() {
            http_req.set_body(req.body);
        }

        cx.http_request(request_id, http_req);
    }

    fn to_git_response(response: &HttpResponse) -> GitHttpResponse {
        let mut headers = Vec::new();
        for (key, values) in &response.headers {
            for value in values {
                headers.push((key.clone(), value.trim().to_string()));
            }
        }

        GitHttpResponse {
            status_code: response.status_code,
            headers,
            body: response.body.clone().unwrap_or_default(),
        }
    }

    fn process_info_refs_response(
        &mut self,
        cx: &mut Cx,
        response: &HttpResponse,
    ) -> Result<(), GitError> {
        let parsed = parse_info_refs_response(&Self::to_git_response(response), None)?;
        let local_head = if self.force_full_checkout {
            None
        } else {
            self.local_head_oid()
        };
        let have: Vec<ObjectId> = local_head.iter().copied().collect();
        let depth = if self.force_full_checkout || have.is_empty() {
            Some(1)
        } else {
            None
        };

        let upload_req = build_upload_pack_request(
            REMOTE_URL,
            parsed.oid,
            &parsed.capabilities,
            &have,
            depth,
        )?;

        self.pending_remote_head = Some(parsed.clone());
        self.phase = SyncPhase::AwaitUploadPack;
        self.send_git_request(cx, upload_req);

        if self.force_full_checkout {
            self.set_status(
                cx,
                &format!(
                    "Checkout: fetching full depth=1 pack at {}...",
                    Self::short_oid(&parsed.oid)
                ),
            );
            self.log_event(
                cx,
                format!(
                    "Checkout: requesting full depth=1 pack at {}",
                    Self::short_oid(&parsed.oid)
                ),
            );
        } else if let Some(local) = local_head {
            self.set_status(
                cx,
                &format!(
                    "Remote moved {} -> {}. Fetching incremental pack...",
                    Self::short_oid(&local),
                    Self::short_oid(&parsed.oid)
                ),
            );
            self.log_event(
                cx,
                format!(
                    "Fetch: remote {} != local {} (have=1, depth=none)",
                    Self::short_oid(&parsed.oid),
                    Self::short_oid(&local)
                ),
            );
        } else {
            self.set_status(
                cx,
                &format!(
                    "No local checkout. Cloning depth=1 at {}...",
                    Self::short_oid(&parsed.oid)
                ),
            );
            self.log_event(
                cx,
                format!(
                    "Fetch: no local HEAD, requesting depth=1 at {}",
                    Self::short_oid(&parsed.oid)
                ),
            );
        }

        Ok(())
    }

    fn process_ls_refs_response(
        &mut self,
        cx: &mut Cx,
        response: &HttpResponse,
    ) -> Result<(), GitError> {
        let parsed = parse_ls_refs_head_response(&Self::to_git_response(response))?;
        let local_head = self.local_head_oid();

        if local_head == Some(parsed.oid) {
            self.phase = SyncPhase::Idle;
            self.set_status(
                cx,
                &format!(
                    "Up to date at {}. Next check in {}s.",
                    Self::short_oid(&parsed.oid),
                    POLL_INTERVAL_SECS as i32
                ),
            );
            self.log_event(
                cx,
                format!("No update: remote HEAD is still {}", Self::short_oid(&parsed.oid)),
            );
            return Ok(());
        }

        self.phase = SyncPhase::Idle;
        match local_head {
            Some(local) => {
                self.set_status(
                    cx,
                    &format!(
                        "Update available {} -> {}. Click Checkout (HTTP).",
                        Self::short_oid(&local),
                        Self::short_oid(&parsed.oid)
                    ),
                );
                self.log_event(
                    cx,
                    format!(
                        "Update available: {} -> {} (waiting for Checkout click)",
                        Self::short_oid(&local),
                        Self::short_oid(&parsed.oid)
                    ),
                );
            }
            None => {
                self.set_status(cx, "No local checkout. Click Checkout (HTTP) to clone.");
                self.log_event(
                    cx,
                    format!(
                        "No local checkout. Remote HEAD {} (waiting for Checkout click)",
                        Self::short_oid(&parsed.oid)
                    ),
                );
            }
        }

        Ok(())
    }

    fn process_upload_pack_response(
        &mut self,
        cx: &mut Cx,
        response: &HttpResponse,
    ) -> Result<(), GitError> {
        let was_full_checkout = self.force_full_checkout;
        let remote_head = self.pending_remote_head.take().ok_or_else(|| {
            GitError::InvalidRef("missing pending remote head for upload-pack phase".to_string())
        })?;

        let Some(pack_data) = extract_pack_from_response(&Self::to_git_response(response))? else {
            self.force_full_checkout = false;
            self.phase = SyncPhase::Idle;
            self.set_status(cx, "No pack data returned. Already up to date.");
            if was_full_checkout {
                self.log_event(cx, "Checkout: no pack data returned; nothing to update");
            } else {
                self.log_event(cx, "Upload-pack returned no pack data; already up to date");
            }
            return Ok(());
        };

        self.log_event(
            cx,
            format!("Applying pack payload ({} bytes)", pack_data.len()),
        );

        let mut hooks = CheckoutLogHooks { files: Vec::new() };
        let report = apply_pack_and_checkout(
            Path::new(DEST_PATH),
            REMOTE_URL,
            remote_head.oid,
            remote_head.ref_name.as_deref(),
            &pack_data,
            &mut hooks,
        )?;

        self.phase = SyncPhase::Idle;
        self.set_status(
            cx,
            &format!(
                "Synced {} | imported {} objs, wrote {} files",
                Self::short_oid(&remote_head.oid),
                report.imported_objects,
                report.checked_out_files
            ),
        );
        self.set_detail(
            cx,
            &format!(
                "Wrote {:.2} MB to worktree",
                report.checked_out_bytes as f64 / (1024.0 * 1024.0)
            ),
        );
        self.log_event(
            cx,
            format!(
                "Sync complete: {} objects imported, {} files checked out",
                report.imported_objects, report.checked_out_files
            ),
        );
        if was_full_checkout {
            self.log_event(
                cx,
                format!(
                    "Checkout complete over HTTP at {}",
                    Self::short_oid(&remote_head.oid)
                ),
            );
        }
        for path in hooks.files {
            self.log_event(cx, format!("checkout {}", path));
        }

        self.force_full_checkout = false;
        Ok(())
    }

    fn fail(&mut self, cx: &mut Cx, message: String) {
        self.phase = SyncPhase::Idle;
        self.active_request_id = None;
        self.pending_remote_head = None;
        self.force_full_checkout = false;
        self.set_status(cx, &format!("Error: {}", message));
        self.log_event(cx, format!("Error: {}", message));
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    poll_timer: Timer,
    #[rust]
    phase: SyncPhase,
    #[rust]
    active_request_id: Option<LiveId>,
    #[rust]
    request_counter: u64,
    #[rust]
    pending_remote_head: Option<RemoteHead>,
    #[rust]
    force_full_checkout: bool,
    #[rust]
    log_counter: u64,
    #[rust]
    last_portal_scroll_state: String,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(remote_value))
            .set_text(cx, REMOTE_URL);
        self.ui
            .label(cx, ids!(dest_value))
            .set_text(cx, &Self::destination_display_path());
        self.set_detail(cx, "Polling GitHub every 30 seconds using HTTPS git endpoints.");
        self.log_event(cx, format!("Startup: remote {}", REMOTE_URL));
        self.log_event(
            cx,
            format!("Startup: destination {}", Self::destination_display_path()),
        );

        self.poll_timer = cx.start_interval(POLL_INTERVAL_SECS);
        self.poll_remote(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(poll_now)).clicked(actions) {
            self.poll_remote(cx);
        }

        if self.ui.button(cx, ids!(checkout_http)).clicked(actions) {
            self.checkout_http(cx);
        }
    }

    fn handle_timer(&mut self, cx: &mut Cx, event: &TimerEvent) {
        if self.poll_timer.is_timer(event).is_some() {
            self.poll_remote(cx);
        }
    }

    fn handle_http_response(&mut self, cx: &mut Cx, request_id: LiveId, response: &HttpResponse) {
        if Some(request_id) != self.active_request_id {
            return;
        }

        self.active_request_id = None;

        let result = match self.phase {
            SyncPhase::AwaitLsRefs => self.process_ls_refs_response(cx, response),
            SyncPhase::AwaitInfoRefs => self.process_info_refs_response(cx, response),
            SyncPhase::AwaitUploadPack => self.process_upload_pack_response(cx, response),
            SyncPhase::Idle => Ok(()),
        };

        if let Err(err) = result {
            self.fail(cx, err.to_string());
        }
    }

    fn handle_http_request_error(&mut self, cx: &mut Cx, request_id: LiveId, err: &HttpError) {
        if Some(request_id) != self.active_request_id {
            return;
        }
        self.fail(cx, err.message.clone());
    }

    fn handle_http_progress(&mut self, cx: &mut Cx, request_id: LiveId, progress: &HttpProgress) {
        if Some(request_id) != self.active_request_id {
            return;
        }

        if self.phase == SyncPhase::AwaitUploadPack {
            self.set_detail(
                cx,
                &format!("Downloading pack: {} / {} bytes", progress.loaded, progress.total),
            );
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        let reason = match event {
            Event::Scroll(_) => Some("scroll"),
            Event::NextFrame(_) => Some("next_frame"),
            _ => None,
        };
        if let Some(reason) = reason {
            self.debug_portal_list_state(cx, reason);
        }
    }
}
