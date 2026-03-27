use crate::*;
use makepad_platform::permission::{Permission, PermissionStatus};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let XrPermissionButton = mod.widgets.ButtonFlat{
        draw_bg +: {
            border_size: 0.0
            border_radius: 0.0
            pixel: fn() {
                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled);
                return Pal.premul(fill)
            }
        }
    }

    mod.widgets.XrPermissionsFlowBase = #(XrPermissionsFlow::register_widget(vm))
    mod.widgets.XrPermissionsFlow = set_type_default() do mod.widgets.XrPermissionsFlowBase{
        width: Fill
        height: Fill
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        padding: Inset{left: 40 right: 40 top: 40 bottom: 40}
        spacing: 18
        show_bg: true
        draw_bg.color: #x06111a

        panel := SolidView{
            width: 620
            height: Fit
            flow: Down
            spacing: 14
            padding: Inset{left: 24 right: 24 top: 22 bottom: 22}
            draw_bg.color: #x09131cdd

            title := H1{
                text: "Mixed Reality Permissions"
                draw_text.color: #xeff7ff
            }

            detail_label := Label{
                width: Fill
                text: "Grant the two Quest permissions below, then use the third button to enter mixed reality."
                draw_text.color: #xb8c8d8
            }

            scene_access_button := XrPermissionButton{
                width: Fill
                text: "Allow Scene Access"
            }

            headset_camera_button := XrPermissionButton{
                width: Fill
                text: "Allow Headset Camera"
            }

            enter_mr_button := XrPermissionButton{
                width: Fill
                text: "Enter Mixed Reality"
            }

            status_label := Label{
                width: Fill
                text: "Checking startup requirements."
                draw_text.color: #x8fe4d6
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrPermissionsFlow {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    scene_access: Option<PermissionStatus>,
    #[rust]
    headset_camera: Option<PermissionStatus>,
    #[rust]
    pending_scene_access_check: Option<i32>,
    #[rust]
    pending_headset_camera_check: Option<i32>,
    #[rust]
    pending_scene_access_request: Option<i32>,
    #[rust]
    pending_headset_camera_request: Option<i32>,
    #[rust]
    ui_refresh_next_frame: Option<NextFrame>,
    #[rust]
    xr_start_next_frame: Option<NextFrame>,
    #[rust]
    hidden_after_start: bool,
}

impl XrPermissionsFlow {
    pub(crate) fn desktop_preflight_visible(&self) -> bool {
        Self::is_android_preflight() && !self.hidden_after_start
    }

    fn is_android_preflight() -> bool {
        cfg!(target_os = "android")
    }

    fn scene_access_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.scene_access, Some(PermissionStatus::Granted))
    }

    fn headset_camera_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.headset_camera, Some(PermissionStatus::Granted))
    }

    fn xr_permissions_ready(&self) -> bool {
        self.scene_access_granted() && self.headset_camera_granted()
    }

    fn permission_checks_pending(&self) -> bool {
        self.pending_scene_access_check.is_some() || self.pending_headset_camera_check.is_some()
    }

    fn permission_requests_pending(&self) -> bool {
        self.pending_scene_access_request.is_some() || self.pending_headset_camera_request.is_some()
    }

    fn scene_access_button_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking Scene Access..."
        } else if self.pending_scene_access_request.is_some() {
            "Waiting For Scene Access..."
        } else if self.scene_access_granted() {
            "Scene Access Granted"
        } else {
            "Allow Scene Access"
        }
    }

    fn headset_camera_button_text(&self) -> &'static str {
        if self.pending_headset_camera_check.is_some() {
            "Checking Headset Camera..."
        } else if self.pending_headset_camera_request.is_some() {
            "Waiting For Headset Camera..."
        } else if self.headset_camera_granted() {
            "Headset Camera Granted"
        } else {
            "Allow Headset Camera"
        }
    }

    fn enter_mr_button_text(&self) -> &'static str {
        if self.hidden_after_start || self.xr_start_next_frame.is_some() {
            "Starting Mixed Reality..."
        } else if self.permission_checks_pending() || self.permission_requests_pending() {
            "Waiting For Permissions..."
        } else if self.xr_permissions_ready() {
            "Enter Mixed Reality"
        } else {
            "Grant Permissions First"
        }
    }

    fn schedule_ui_refresh(&mut self, cx: &mut Cx) {
        self.ui_refresh_next_frame = Some(cx.new_next_frame());
        self.redraw(cx);
    }

    fn detail_text(&self) -> &'static str {
        if self.hidden_after_start || self.xr_start_next_frame.is_some() {
            "Quest scene access and headset camera are granted. Starting mixed reality."
        } else if self.xr_permissions_ready() {
            "Both Quest permissions are granted. Press Enter Mixed Reality when you are ready."
        } else if !self.scene_access_granted() {
            "Allow Scene Access first. This unlocks environment depth and passthrough occlusion."
        } else if !self.headset_camera_granted() {
            "Allow Headset Camera next. This unlocks the passthrough texture overlay."
        } else {
            "Grant the required Quest permissions before entering mixed reality."
        }
    }

    fn status_text(&self) -> &'static str {
        if self.permission_checks_pending() {
            "Checking current Quest permission status."
        } else if self.hidden_after_start || self.xr_start_next_frame.is_some() {
            "Quest permissions granted. Entering mixed reality."
        } else if self.permission_requests_pending() {
            "Approve the Quest permission dialog to continue."
        } else if self.xr_permissions_ready() {
            "Quest scene access and headset camera granted. Ready to enter mixed reality."
        } else if !self.scene_access_granted() {
            "Quest scene access has not been granted yet."
        } else if !self.headset_camera_granted() {
            "Quest headset camera permission has not been granted yet."
        } else {
            "Quest permissions are incomplete."
        }
    }

    fn refresh_ui(&mut self, cx: &mut Cx) {
        self.label(cx, ids!(detail_label))
            .set_text(cx, self.detail_text());
        self.label(cx, ids!(status_label))
            .set_text(cx, self.status_text());
        let scene_access_button = self.button(cx, ids!(scene_access_button));
        scene_access_button.set_enabled(
            cx,
            self.pending_scene_access_check.is_none()
                && self.pending_scene_access_request.is_none()
                && !self.scene_access_granted(),
        );
        self.widget(cx, ids!(scene_access_button))
            .set_text(cx, self.scene_access_button_text());

        let headset_camera_button = self.button(cx, ids!(headset_camera_button));
        headset_camera_button.set_enabled(
            cx,
            self.pending_headset_camera_check.is_none()
                && self.pending_headset_camera_request.is_none()
                && !self.headset_camera_granted(),
        );
        self.widget(cx, ids!(headset_camera_button))
            .set_text(cx, self.headset_camera_button_text());

        let enter_mr_button = self.button(cx, ids!(enter_mr_button));
        enter_mr_button.set_enabled(
            cx,
            self.xr_permissions_ready()
                && !self.hidden_after_start
                && self.xr_start_next_frame.is_none(),
        );
        self.widget(cx, ids!(enter_mr_button))
            .set_text(cx, self.enter_mr_button_text());
    }

    fn begin_scene_access_check(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight() || self.pending_scene_access_check.is_some() {
            return;
        }
        self.pending_scene_access_check = Some(cx.check_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn begin_headset_camera_check(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight() || self.pending_headset_camera_check.is_some() {
            return;
        }
        self.pending_headset_camera_check = Some(cx.check_permission(Permission::HeadsetCamera));
        self.schedule_ui_refresh(cx);
    }

    fn request_scene_access(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
            || self.pending_scene_access_request.is_some()
        {
            return;
        }
        self.pending_scene_access_request = Some(cx.request_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn request_headset_camera(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight()
            || self.pending_headset_camera_check.is_some()
            || self.pending_headset_camera_request.is_some()
        {
            return;
        }
        self.pending_headset_camera_request =
            Some(cx.request_permission(Permission::HeadsetCamera));
        crate::log!(
            "XrPermissionsFlow request_headset_camera request_id={:?}",
            self.pending_headset_camera_request
        );
        self.schedule_ui_refresh(cx);
    }

    fn begin_permission_checks(&mut self, cx: &mut Cx) {
        self.begin_scene_access_check(cx);
        self.begin_headset_camera_check(cx);
    }

    fn start_xr(&mut self, cx: &mut Cx) {
        self.hidden_after_start = true;
        self.xr_start_next_frame = Some(cx.new_next_frame());
        self.redraw(cx);
    }

    fn maybe_start_xr(&mut self, cx: &mut Cx) {
        if self.xr_permissions_ready()
            && !self.hidden_after_start
            && self.xr_start_next_frame.is_none()
        {
            self.start_xr(cx);
        }
    }
}

impl Widget for XrPermissionsFlow {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !Self::is_android_preflight() {
            return;
        }

        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            if self.button(cx, ids!(scene_access_button)).clicked(actions) {
                self.request_scene_access(cx);
            }
            if self
                .button(cx, ids!(headset_camera_button))
                .clicked(actions)
            {
                self.request_headset_camera(cx);
            }
            if self.button(cx, ids!(enter_mr_button)).clicked(actions)
                && self.xr_permissions_ready()
                && !self.hidden_after_start
                && self.xr_start_next_frame.is_none()
            {
                self.start_xr(cx);
            }
        }

        match event {
            Event::Startup => {
                self.schedule_ui_refresh(cx);
                self.begin_permission_checks(cx);
            }
            Event::NextFrame(ne) => {
                if self
                    .ui_refresh_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.ui_refresh_next_frame = None;
                    self.refresh_ui(cx);
                }

                if self
                    .xr_start_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.xr_start_next_frame = None;
                    cx.xr_start_presenting();
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::SceneAccess => {
                if self.pending_scene_access_check == Some(result.request_id) {
                    self.pending_scene_access_check = None;
                } else if self.pending_scene_access_request == Some(result.request_id) {
                    self.pending_scene_access_request = None;
                } else {
                    return;
                }
                self.scene_access = Some(result.status);
                self.maybe_start_xr(cx);
                self.schedule_ui_refresh(cx);
            }
            Event::PermissionResult(result) if result.permission == Permission::HeadsetCamera => {
                if self.pending_headset_camera_check == Some(result.request_id) {
                    self.pending_headset_camera_check = None;
                } else if self.pending_headset_camera_request == Some(result.request_id) {
                    self.pending_headset_camera_request = None;
                } else {
                    return;
                }
                self.headset_camera = Some(result.status);
                self.maybe_start_xr(cx);
                self.schedule_ui_refresh(cx);
            }
            Event::Resume if !self.permission_requests_pending() => {
                self.begin_permission_checks(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.desktop_preflight_visible() {
            return DrawStep::done();
        }
        self.view.draw_walk(cx, scope, walk)
    }
}
