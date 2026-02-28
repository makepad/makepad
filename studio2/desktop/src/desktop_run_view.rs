use crate::{app::AppData, makepad_widgets::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopRunViewBase = #(DesktopRunView::register_widget(vm))

    mod.widgets.DesktopRunView = set_type_default() do mod.widgets.DesktopRunViewBase {
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        padding: Inset {left: 14.0 right: 14.0 top: 14.0 bottom: 14.0}

        run_title := H2 {
            width: Fill
            text: "Run"
        }

        run_status := Label {
            width: Fill
            text: "Press play in the run list to start a package."
            draw_text.color: #x89A0C7
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DesktopRunView {
    #[deref]
    view: View,
}

impl Widget for DesktopRunView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let path = cx.widget_tree().path_to(self.widget_uid());
        let tab_id = path
            .get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(run_first));

        let mut title = "Run".to_string();
        let mut status = "Press play in the run list to start a package.".to_string();
        if let Some(data) = scope.data.get_mut::<AppData>() {
            if let Some(tab) = data.run_tab_state.get(&tab_id) {
                title = format!("{} ({})", tab.package, tab.mount);
                status = format!("status: {}", tab.status);
            }
        }
        self.view.label(cx, ids!(run_title)).set_text(cx, &title);
        self.view.label(cx, ids!(run_status)).set_text(cx, &status);
        self.view.draw_walk_all(cx, scope, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}
