use crate::{app_data::AppData, makepad_widgets::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopModelPickerBase = #(DesktopModelPicker::register_widget(vm))

    mod.widgets.ModelPickerItem = View {
        width: Fill
        height: 56.0
        flow: Overlay
        show_bg: true

        draw_bg +: {
            is_selected: instance(0.0)
            is_even: instance(0.0)
            pixel: fn() {
                let base = #x101114.mix(#x14161A, self.is_even)
                let selected_color = #x232936
                return base.mix(selected_color, self.is_selected);
            }
        }

        content := View {
            width: Fill
            height: Fill
            flow: Right
            align: Align {x: 0.0 y: 0.5}
            spacing: theme.space_2
            padding: Inset {left: 12.0 right: 12.0 top: 0.0 bottom: 0.0}

            icon := Label {
                width: 20.0
                text: ""
                draw_text +: {
                    font_size: 14.0
                    text_style: theme.font_bold
                }
            }

            text_layout := View {
                width: Fill
                height: Fit
                flow: Down
                spacing: 1.0

                label := Label {
                    width: Fill
                    text: ""
                    draw_text +: {
                        color: theme.color_label_outer
                        font_size: 11.0
                        text_style: theme.font_bold
                    }
                }

                detail := Label {
                    width: Fill
                    text: ""
                    draw_text +: {
                        color: theme.color_label_inner_inactive
                        font_size: 9.0
                        text_style: theme.font_regular
                    }
                }
            }

            badge := RoundedView {
                width: Fit
                height: Fit
                padding: Inset {left: 6.0 right: 6.0 top: 2.0 bottom: 2.0}
                show_bg: true
                draw_bg +: {
                    color: #x242730
                    radius: 4.0
                }
                badge_text := Label {
                    width: Fit
                    text: ""
                    draw_text +: {
                        color: theme.color_label_inner
                        font_size: 8.5
                        text_style: theme.font_bold
                    }
                }
            }
        }

        row_button := ButtonFlat {
            width: Fill
            height: Fill
            text: ""
            draw_bg +: {
                color: #0000
                color_hover: vec4(1.0, 1.0, 1.0, 0.04)
                color_down: vec4(1.0, 1.0, 1.0, 0.08)
                color_focus: #0000
                color_disabled: #0000
                border_color: #0000
                border_color_hover: #0000
                border_color_down: #0000
                border_color_focus: #0000
                border_color_disabled: #0000
                border_size: 0.0
            }
            draw_text +: {
                color: #0000
                color_hover: #0000
                color_down: #0000
                color_focus: #0000
                color_disabled: #0000
            }
        }
    }

    mod.widgets.ModelPickerEmpty = View {
        width: Fill
        height: 56.0
        show_bg: true
        draw_bg +: {
            is_even: instance(0.0)
            pixel: fn() {
                return #x101114.mix(#x14161A, self.is_even)
            }
        }
        padding: Inset {left: 12.0 right: 12.0 top: 8.0 bottom: 8.0}
        info_label := Label {
            width: Fill
            text: ""
            draw_text.color: theme.color_label_inner_inactive
        }
    }

    mod.widgets.DesktopModelPicker = set_type_default() do mod.widgets.DesktopModelPickerBase {
        width: Fill
        height: Fill
        flow: Down

        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            max_pull_down: 0.0
            capture_overload: false
            grab_key_focus: false
            auto_tail: false
            selectable: false
            drag_scrolling: true
            Item := mod.widgets.ModelPickerItem {}
            Empty := mod.widgets.ModelPickerEmpty {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopModelPickerAction {
    Select {
        backend_id: String,
    },
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum ModelPickerRowData {
    Backend {
        backend_id: String,
    },
    #[default]
    None,
}

impl ActionDefaultRef for ModelPickerRowData {
    fn default_ref() -> &'static Self {
        &ModelPickerRowData::None
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DesktopModelPicker {
    #[deref]
    view: View,
}

impl DesktopModelPicker {
    const ROW_HEIGHT: f64 = 56.0;

    fn empty_fill_rows(list: &PortalList, cx: &Cx2d, used_rows: usize) -> usize {
        let viewport_h = list.area().rect(cx).size.y.max(0.0);
        if viewport_h <= 0.0 {
            return 1usize.saturating_sub(used_rows);
        }
        let visible_rows = ((viewport_h / Self::ROW_HEIGHT).ceil() as usize).max(1);
        visible_rows.saturating_sub(used_rows)
    }

    fn draw_entries(&mut self, cx: &mut Cx2d, list: &mut PortalList, data: &AppData) {
        let Some(active_mount) = data.active_mount.as_ref() else {
            self.draw_empty(cx, list, "No active mount");
            return;
        };
        let Some(mount_state) = data.mounts.get(active_mount) else {
            self.draw_empty(cx, list, "No mount state");
            return;
        };
        let Some(state) = mount_state.ai_state.as_ref() else {
            self.draw_empty(cx, list, "No AI state");
            return;
        };

        let filter = data.model_search_filter.to_lowercase();
        let filtered_backends: Vec<_> = state
            .backends
            .iter()
            .filter(|backend| {
                filter.is_empty()
                    || backend.label.to_lowercase().contains(&filter)
                    || backend.detail.to_lowercase().contains(&filter)
            })
            .collect();

        if filtered_backends.is_empty() {
            self.draw_empty(cx, list, "No matching models");
            return;
        }

        list.set_item_range(cx, 0, filtered_backends.len());

        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };

            let Some(backend) = filtered_backends.get(item_id) else { continue; };

            let is_selected_f = if Some(&backend.id) == state.active_backend_id.as_ref() {
                1.0
            } else {
                0.0
            };

            let mut item = list.item(cx, item_id, id!(Item)).as_view();
            script_apply_eval!(cx, item, {
                draw_bg +: {
                    is_selected: #(is_selected_f)
                    is_even: #(is_even_f)
                }
            });

            // Map backend ID to emoji icon
            let icon_str = if backend.id.contains("gemini") || backend.id.contains("google") {
                "✨"
            } else if backend.id.contains("claude") || backend.id.contains("anthropic") {
                "🧠"
            } else if backend.id.contains("openai") || backend.id.contains("gpt") {
                "🤖"
            } else if backend.id.contains("local") || backend.id.contains("ollama") {
                "💻"
            } else {
                "🤖"
            };

            item.label(cx, ids!(icon)).set_text(cx, icon_str);
            item.label(cx, ids!(label)).set_text(cx, &backend.label);
            item.label(cx, ids!(detail)).set_text(cx, &backend.detail);

            let badge_view = item.view(cx, ids!(badge));
            let badge_text = item.label(cx, ids!(badge_text));
            if Some(&backend.id) == state.active_backend_id.as_ref() {
                badge_view.set_visible(cx, true);
                badge_text.set_text(cx, "Active");
            } else if backend.configured {
                badge_view.set_visible(cx, true);
                badge_text.set_text(cx, "Ready");
            } else {
                badge_view.set_visible(cx, true);
                badge_text.set_text(cx, "Configure");
            }

            let button = item.button(cx, ids!(row_button));
            button.set_action_data(ModelPickerRowData::Backend {
                backend_id: backend.id.clone(),
            });

            item.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_empty(&mut self, cx: &mut Cx2d, list: &mut PortalList, text: &str) {
        let rows = Self::empty_fill_rows(list, cx, 0).max(1);
        list.set_item_range(cx, 0, rows);
        while let Some(item_id) = list.next_visible_item(cx) {
            let mut item = list.item(cx, item_id, id!(Empty)).as_view();
            let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };
            script_apply_eval!(cx, item, {
                draw_bg +: {is_even: #(is_even_f)}
            });
            let label = if item_id == 0 { text } else { "" };
            item.label(cx, ids!(info_label)).set_text(cx, label);
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for DesktopModelPicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                if let Some(data) = scope.data.get_mut::<AppData>() {
                    self.draw_entries(cx, &mut *list, data);
                } else {
                    self.draw_empty(cx, &mut *list, "No app state");
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let list = self.view.portal_list(cx, ids!(list));
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if !list.any_items_with_actions(actions) {
                return;
            }
            for (_item_id, item) in list.items_with_actions(actions) {
                let button = item.button(cx, ids!(row_button));
                if button.clicked(actions) {
                    if let ModelPickerRowData::Backend { backend_id } = button.action_data().cast_ref() {
                        cx.widget_action(
                            uid,
                            DesktopModelPickerAction::Select {
                                backend_id: backend_id.clone(),
                            },
                        );
                    }
                }
            }
        }
    }
}

impl DesktopModelPickerRef {
    pub fn select_requested(&self, actions: &Actions) -> Option<String> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DesktopModelPickerAction::Select { backend_id } = item.cast() {
                return Some(backend_id);
            }
        }
        None
    }
}
