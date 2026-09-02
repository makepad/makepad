//! Shared local-model install rows, licence ceremony, and install controller.

use makepad_ai_hub::license::LicensePrompt;
use makepad_ai_hub::local::{InstallHandle, InstallMsg, InstallState, LocalModels};
use makepad_ai_hub::registry::LicenseRestriction;
use makepad_widgets::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ModelInstallRowBase = #(ModelInstallRow::register_widget(vm))
    mod.widgets.ModelInstallRow = set_type_default() do mod.widgets.ModelInstallRowBase {
        width: Fill
        height: Fit
        flow: Down
        spacing: 6
        margin: Inset{bottom: 6}
        padding: Inset{left: 12 right: 12 top: 10 bottom: 10}
        show_bg: true
        draw_bg +: {
            color: #x20242b
        }

        View {
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{x: 0.0 y: 0.5}
            model_name := Label {
                width: Fill
                height: Fit
                text: ""
                draw_text +: {
                    color: #xe8edf4
                    text_style: theme.font_bold{font_size: 12}
                }
            }
            model_size := Label {
                width: Fit
                height: Fit
                text: "0 MB"
                draw_text +: {
                    color: #x8f9baa
                    text_style: theme.font_regular{font_size: 10}
                }
            }
        }

        View {
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{x: 0.0 y: 0.5}
            licence_link := LinkLabel {
                width: Fit
                height: Fit
                text: "Licence"
                draw_text +: { text_style: theme.font_regular{font_size: 10} }
            }
            restriction_chip := RoundedView {
                width: Fit
                height: Fit
                padding: Inset{left: 6 right: 6 top: 2 bottom: 2}
                show_bg: true
                draw_bg +: {
                    color: #x343a44
                    radius: 8.0
                }
                restriction := Label {
                    width: Fit
                    height: Fit
                    text: "restricted"
                    draw_text +: {
                        color: #xc7cfda
                        text_style: theme.font_regular{font_size: 9}
                    }
                }
            }
        }

        View {
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{x: 0.0 y: 0.5}
            state_text := Label {
                width: Fill
                height: Fit
                text: "not installed"
                draw_text +: {
                    color: #xaab4c2
                    text_style: theme.font_regular{font_size: 10}
                }
            }
            install_button := Button {
                width: 92
                height: 26
                text: "INSTALL"
            }
        }
    }

    mod.widgets.LicenseModalBase = #(LicenseModal::register_widget(vm))
    mod.widgets.LicenseModal = set_type_default() do mod.widgets.LicenseModalBase {
        modal := Modal {
            can_dismiss: false
            content +: {
                width: 540
                height: Fit
                card := RoundedView {
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 10
                    padding: 20
                    show_bg: true
                    draw_bg +: {
                        color: #x16161b
                        border_color: #xffffff18
                        border_size: 1.0
                        radius: 6.0
                    }
                    licence_title := Label {
                        width: Fill
                        height: Fit
                        text: "Before downloading model"
                        draw_text +: {
                            color: #xf2f4f8
                            text_style: theme.font_bold{font_size: 13}
                        }
                    }
                    licence_name := Label {
                        width: Fill
                        height: Fit
                        text: ""
                        draw_text +: { color: #xc8d0dc }
                    }
                    restriction_text := Label {
                        width: Fill
                        height: Fit
                        text: ""
                        draw_text +: {
                            color: #xe2b982
                            text_style: theme.font_regular{font_size: 10}
                        }
                    }
                    licence_summary := Label {
                        width: Fill
                        height: Fit
                        text: ""
                        draw_text +: {
                            color: #xaeb8c6
                            text_style: theme.font_regular{font_size: 11}
                        }
                    }
                    full_licence := LinkLabel {
                        text: "Read the full licence"
                    }
                    View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{x: 1.0 y: 0.5}
                        decline := Button { width: 90 height: 28 text: "Decline" }
                        accept := Button { width: 90 height: 28 text: "Accept" }
                    }
                }
            }
        }
    }

    mod.widgets.ModelInstallPanelBase = #(ModelInstallPanel::register_widget(vm))
    mod.widgets.ModelInstallPanel = set_type_default() do mod.widgets.ModelInstallPanelBase {
        width: Fill
        height: Fill
        flow: Overlay

        empty := Label {
            width: Fill
            height: Fit
            text: "no models registered for this app"
            draw_text +: {
                color: #x8f9baa
                text_style: theme.font_regular{font_size: 11}
            }
        }
        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            Row := mod.widgets.ModelInstallRow {}
        }
        licence_modal := mod.widgets.LicenseModal {}
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ModelRowInstallState {
    #[default]
    NotInstalled,
    Downloading,
    Installed,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRowState {
    pub model_id: String,
    pub name: String,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub state: ModelRowInstallState,
    pub license_name: String,
    pub restriction: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ModelInstallAction {
    Install(String),
    Cancel(String),
    OpenLicense(String),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ModelInstallRow {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    state: ModelRowState,
}

impl ModelInstallRow {
    pub fn set_state(&mut self, cx: &mut Cx, state: ModelRowState) {
        self.state = state;
        self.sync(cx);
    }

    fn sync(&mut self, cx: &mut Cx) {
        self.view
            .label(cx, ids!(model_name))
            .set_text(cx, &self.state.name);
        self.view
            .label(cx, ids!(model_size))
            .set_text(cx, &format_mb(self.state.bytes_total));
        let link = self.view.link_label(cx, ids!(licence_link));
        link.set_text(cx, &self.state.license_name);
        self.view
            .label(cx, ids!(restriction))
            .set_text(
                cx,
                if self.state.restriction == "none" {
                    "permissive"
                } else {
                    &self.state.restriction
                },
            );

        let (status, button, visible) = match &self.state.state {
            ModelRowInstallState::NotInstalled => ("not installed".to_string(), "INSTALL", true),
            ModelRowInstallState::Downloading => {
                let total = self.state.bytes_total.max(1);
                (
                    format!(
                        "downloading {}% · {}/{} MB",
                        (self.state.bytes_done.saturating_mul(100) / total).min(100),
                        self.state.bytes_done / 1_000_000,
                        self.state.bytes_total / 1_000_000
                    ),
                    "CANCEL",
                    true,
                )
            }
            ModelRowInstallState::Installed => ("installed".to_string(), "INSTALL", false),
            ModelRowInstallState::Failed(error) => {
                (format!("not installed · {error}"), "INSTALL", true)
            }
        };
        self.view
            .label(cx, ids!(state_text))
            .set_text(cx, &status);
        let install = self.view.button(cx, ids!(install_button));
        install.set_text(cx, button);
        install.set_visible(cx, visible);
        self.view.redraw(cx);
    }
}

impl Widget for ModelInstallRow {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        if self.view.button(cx, ids!(install_button)).clicked(actions) {
            let action = if matches!(self.state.state, ModelRowInstallState::Downloading) {
                ModelInstallAction::Cancel(self.state.model_id.clone())
            } else {
                ModelInstallAction::Install(self.state.model_id.clone())
            };
            cx.widget_action(self.widget_uid(), action);
        }
        if self.view.link_label(cx, ids!(licence_link)).clicked(actions) {
            cx.widget_action(
                self.widget_uid(),
                ModelInstallAction::OpenLicense(self.state.model_id.clone()),
            );
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl ModelInstallRowRef {
    pub fn set_state(&self, cx: &mut Cx, state: ModelRowState) {
        if let Some(mut row) = self.borrow_mut() {
            row.set_state(cx, state);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LicenseModalAction {
    Accepted(String),
    Declined(String),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct LicenseModal {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    model_id: String,
}

impl LicenseModal {
    pub fn show(&mut self, cx: &mut Cx, prompt: &LicensePrompt) {
        self.model_id = prompt.model_id.clone();
        self.view
            .label(cx, ids!(licence_title))
            .set_text(cx, &format!("Before downloading {}", prompt.model_id));
        self.view
            .label(cx, ids!(licence_name))
            .set_text(cx, &prompt.name);
        self.view
            .label(cx, ids!(restriction_text))
            .set_text(cx, restriction_text(prompt.restriction));
        self.view
            .label(cx, ids!(licence_summary))
            .set_text(cx, &prompt.summary);
        let link = self.view.link_label(cx, ids!(full_licence));
        link.set_text(cx, "Read the full licence");
        link.set_url(&prompt.url);
        self.view.modal(cx, ids!(modal)).open(cx);
        self.view.redraw(cx);
    }

    fn close(&mut self, cx: &mut Cx) {
        self.view.modal(cx, ids!(modal)).close(cx);
        self.view.redraw(cx);
    }
}

impl Widget for LicenseModal {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        if self.view.button(cx, ids!(accept)).clicked(actions) {
            let model_id = self.model_id.clone();
            self.close(cx);
            cx.widget_action(self.widget_uid(), LicenseModalAction::Accepted(model_id));
        } else if self.view.button(cx, ids!(decline)).clicked(actions) {
            let model_id = self.model_id.clone();
            self.close(cx);
            cx.widget_action(self.widget_uid(), LicenseModalAction::Declined(model_id));
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl LicenseModalRef {
    pub fn show(&self, cx: &mut Cx, prompt: &LicensePrompt) {
        if let Some(mut modal) = self.borrow_mut() {
            modal.show(cx, prompt);
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ModelInstallPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    rows: Vec<ModelRowState>,
    #[rust]
    installs: HashMap<String, InstallHandle>,
    #[rust]
    pending: Vec<PanelAction>,
    #[rust]
    install_after_accept: Option<String>,
}

#[derive(Clone, Debug)]
enum PanelAction {
    Model(ModelInstallAction),
    License(LicenseModalAction),
}

impl ModelInstallPanel {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<ModelRowState>) {
        self.rows = rows;
        self.sync_empty(cx);
        self.view.redraw(cx);
    }

    pub fn rows(&self) -> &[ModelRowState] {
        &self.rows
    }

    /// Drain UI intentions and installer workers without blocking the UI
    /// thread. Calling this from the host's normal event/frame pump is the
    /// only controller glue an embedding app needs.
    pub fn pump(&mut self, cx: &mut Cx, models: &mut LocalModels) {
        for action in std::mem::take(&mut self.pending) {
            match action {
                PanelAction::Model(ModelInstallAction::Install(model_id)) => {
                    if self.installs.contains_key(&model_id) {
                        continue;
                    }
                    if models.license_acknowledged(&model_id) {
                        self.begin_install(cx, models, &model_id);
                    } else if let Some(prompt) = models.license(&model_id) {
                        self.install_after_accept = Some(model_id);
                        self.show_license(cx, &prompt);
                    }
                }
                PanelAction::Model(ModelInstallAction::Cancel(model_id)) => {
                    if let Some(handle) = self.installs.get(&model_id) {
                        handle.cancel();
                    }
                }
                PanelAction::Model(ModelInstallAction::OpenLicense(model_id)) => {
                    self.install_after_accept = None;
                    if let Some(prompt) = models.license(&model_id) {
                        self.show_license(cx, &prompt);
                    }
                }
                PanelAction::Model(ModelInstallAction::None) => {}
                PanelAction::License(LicenseModalAction::Accepted(model_id)) => {
                    match models.acknowledge_license(&model_id) {
                        Ok(()) => {
                            if self.install_after_accept.as_deref() == Some(model_id.as_str()) {
                                self.begin_install(cx, models, &model_id);
                            }
                        }
                        Err(error) => self.fail_row(&model_id, error.to_string()),
                    }
                    self.install_after_accept = None;
                }
                PanelAction::License(LicenseModalAction::Declined(_)) => {
                    self.install_after_accept = None;
                }
                PanelAction::License(LicenseModalAction::None) => {}
            }
        }

        let model_ids: Vec<String> = self.installs.keys().cloned().collect();
        let mut finished = Vec::new();
        for model_id in model_ids {
            let messages = self
                .installs
                .get(&model_id)
                .map(InstallHandle::poll)
                .unwrap_or_default();
            for message in messages {
                match message {
                    InstallMsg::Progress { .. } | InstallMsg::FileDone { .. } => {
                        self.update_progress_from_disk(models, &model_id);
                    }
                    InstallMsg::Finished => {
                        finished.push(model_id.clone());
                    }
                    InstallMsg::Failed(error) => self.fail_row(&model_id, error),
                    InstallMsg::Cancelled => {
                        finished.push(model_id.clone());
                    }
                }
            }
        }
        for model_id in finished {
            self.installs.remove(&model_id);
            self.update_progress_from_disk(models, &model_id);
        }
        self.sync_empty(cx);
        self.view.redraw(cx);
    }

    fn begin_install(&mut self, cx: &mut Cx, models: &LocalModels, model_id: &str) {
        match models.start_install(model_id) {
            Ok(handle) => {
                self.installs.insert(model_id.to_string(), handle);
                if let Some(row) = self.row_mut(model_id) {
                    row.state = ModelRowInstallState::Downloading;
                }
            }
            Err(error) => self.fail_row(model_id, error.to_string()),
        }
        self.view.redraw(cx);
    }

    fn update_progress_from_disk(&mut self, models: &LocalModels, model_id: &str) {
        let active = self.installs.contains_key(model_id);
        let state = models.install_state(model_id);
        if let Some(row) = self.row_mut(model_id) {
            let preserve_failure =
                !active && matches!(&row.state, ModelRowInstallState::Failed(_));
            match state {
                InstallState::NotInstalled { bytes_total } => {
                    row.bytes_done = 0;
                    row.bytes_total = bytes_total;
                    if !preserve_failure {
                        row.state = if active {
                            ModelRowInstallState::Downloading
                        } else {
                            ModelRowInstallState::NotInstalled
                        };
                    }
                }
                InstallState::Partial {
                    bytes_done,
                    bytes_total,
                } => {
                    row.bytes_done = bytes_done;
                    row.bytes_total = bytes_total;
                    if !preserve_failure {
                        row.state = if active {
                            ModelRowInstallState::Downloading
                        } else {
                            ModelRowInstallState::NotInstalled
                        };
                    }
                }
                InstallState::Installed => {
                    row.bytes_done = row.bytes_total;
                    row.state = ModelRowInstallState::Installed;
                }
            }
        }
    }

    fn fail_row(&mut self, model_id: &str, error: String) {
        if let Some(row) = self.row_mut(model_id) {
            row.state = ModelRowInstallState::Failed(error);
        }
    }

    fn row_mut(&mut self, model_id: &str) -> Option<&mut ModelRowState> {
        self.rows.iter_mut().find(|row| row.model_id == model_id)
    }

    fn show_license(&self, cx: &mut Cx, prompt: &LicensePrompt) {
        let widget = self.view.widget(cx, ids!(licence_modal));
        if let Some(mut modal) = widget.borrow_mut::<LicenseModal>() {
            modal.show(cx, prompt);
        };
    }

    fn sync_empty(&self, cx: &mut Cx) {
        let empty = self.rows.is_empty();
        self.view.label(cx, ids!(empty)).set_visible(cx, empty);
        self.view.portal_list(cx, ids!(list)).set_visible(cx, !empty);
    }
}

impl Widget for ModelInstallPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let list = self.view.portal_list(cx, ids!(list));
        for (_, item) in list.items_with_actions(actions) {
            let model = actions.find_widget_action_cast::<ModelInstallAction>(item.widget_uid());
            if !matches!(model, ModelInstallAction::None) {
                self.pending.push(PanelAction::Model(model));
            }
        }
        let modal_uid = self.view.widget(cx, ids!(licence_modal)).widget_uid();
        let license = actions.find_widget_action_cast::<LicenseModalAction>(modal_uid);
        if !matches!(license, LicenseModalAction::None) {
            self.pending.push(PanelAction::License(license));
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else {
                continue;
            };
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(item_id) = list.next_visible_item(cx) {
                let row_widget = list.item(cx, item_id, id!(Row));
                if let (Some(state), Some(mut row)) = (
                    self.rows.get(item_id).cloned(),
                    row_widget.borrow_mut::<ModelInstallRow>(),
                ) {
                    row.set_state(cx, state);
                }
                row_widget.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }
}

impl ModelInstallPanelRef {
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<ModelRowState>) {
        if let Some(mut panel) = self.borrow_mut() {
            panel.set_rows(cx, rows);
        }
    }

    pub fn pump(&self, cx: &mut Cx, models: &mut LocalModels) {
        if let Some(mut panel) = self.borrow_mut() {
            panel.pump(cx, models);
        }
    }
}

fn format_mb(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1_000_000.0)
}

fn restriction_text(restriction: LicenseRestriction) -> &'static str {
    match restriction {
        LicenseRestriction::None => {
            "Permissive weight licence. Acknowledgement is still required to clear the model."
        }
        LicenseRestriction::NonCommercial => {
            "Non-commercial weights. Personal / research use only."
        }
        LicenseRestriction::Community => {
            "Community licence. Read the terms before any product use."
        }
        LicenseRestriction::Restricted => {
            "Restricted licence. Review the full terms before use."
        }
    }
}
