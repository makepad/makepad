use crate::{app_data::AppData, makepad_widgets::*};
use makepad_studio_protocol::hub_protocol::QueryId;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopRunListBase = #(DesktopRunList::register_widget(vm))

    mod.widgets.RunPlayIcon = View {
        width: 14.0
        height: 14.0
        margin: Inset {left: 3.0 right: 3.0 top: 0.0 bottom: 0.0}
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.move_to(3.0, 2.0)
                sdf.line_to(11.0, 7.0)
                sdf.line_to(3.0, 12.0)
                sdf.close_path()
                sdf.fill(theme.color_label_inner.mix(#xFFFFFF, self.hover))
                return sdf.result
            }
        }
    }

    mod.widgets.RunStopIcon = View {
        width: 14.0
        height: 14.0
        margin: Inset {left: 3.0 right: 3.0 top: 0.0 bottom: 0.0}
        show_bg: true
        visible: false
        draw_bg +: {
            hover: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(3.0, 3.0, 8.0, 8.0, 0.0)
                sdf.fill(#xE38AA6.mix(#xFFD2E1, self.hover))
                return sdf.result
            }
        }
    }

    mod.widgets.RunListItem = View {
        width: Fill
        height: 34.0
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        spacing: theme.space_1
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        show_bg: true

        draw_bg +: {
            is_even: instance(0.0)
            is_running: instance(0.0)
            pixel: fn() {
                let base = theme.color_bg_even.mix(
                    theme.color_bg_odd,
                    self.is_even
                )
                return base.mix(#xFFFFFF, self.is_running * 0.035)
            }
        }

        animator: Animator {
            ignore_missing: true
            hover: {
                default: @off
                off: AnimatorState {
                    from: {all: Forward {duration: 0.08}}
                    apply: {
                        play_icon: {draw_bg: {hover: 0.0}}
                        stop_icon: {draw_bg: {hover: 0.0}}
                        row_button: {draw_text: {hover: 0.0}}
                    }
                }
                on: AnimatorState {
                    cursor: MouseCursor.Hand
                    from: {all: Snap}
                    apply: {
                        play_icon: {draw_bg: {hover: 1.0}}
                        stop_icon: {draw_bg: {hover: 1.0}}
                        row_button: {draw_text: {hover: 1.0}}
                    }
                }
            }
        }

        icon_wrap := View {
            width: Fit
            height: Fit
            flow: Overlay
            play_icon := mod.widgets.RunPlayIcon {}
            stop_icon := mod.widgets.RunStopIcon {}
        }

        row_button := ButtonFlat {
            width: Fill
            height: Fill
            align: Align {x: 0.0 y: 0.5}
            label_walk: Walk {width: Fit height: Fit}
            padding: Inset {left: 4.0 right: 0.0 top: 0.0 bottom: 0.0}
            text: ""
            draw_bg +: {
                color: #0000
                color_hover: #0000
                color_pressed: #0000
                color_focus: #0000
                color_disabled: #0000
                border_color: #0000
                border_color_hover: #0000
                border_color_pressed: #0000
                border_color_focus: #0000
                border_color_disabled: #0000
            }
            draw_text +: {
                color: theme.color_label_inner
                color_hover: #xFFFFFF
                color_pressed: #xFFFFFF
                color_focus: #xFFFFFF
            }
        }

        status_badge := View {
            width: Fit
            height: Fit
            visible: false
            padding: Inset {left: 7.0 right: 7.0 top: 3.0 bottom: 3.0}
            margin: Inset {left: 8.0 right: 0.0 top: 0.0 bottom: 0.0}
            show_bg: true
            draw_bg +: {
                color: #x2C3130
                border_radius: 9.0
            }
            badge_text := Label {
                width: Fit
                text: "Running"
                draw_text +: {
                    color: #xA9CDB4
                }
            }
        }
    }

    mod.widgets.RunListEmpty = View {
        width: Fill
        height: 34.0
        show_bg: true
        draw_bg +: {
            is_even: instance(0.0)
            pixel: fn() {
                return theme.color_bg_even.mix(
                    theme.color_bg_odd,
                    self.is_even
                )
            }
        }
        padding: Inset {left: 10.0 right: 10.0 top: 8.0 bottom: 8.0}
        info_label := Label {
            width: Fill
            text: ""
            draw_text.color: #x89A0C7
        }
    }

    mod.widgets.DesktopRunList = set_type_default() do mod.widgets.DesktopRunListBase {
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
            Item := mod.widgets.RunListItem {}
            Empty := mod.widgets.RunListEmpty {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopRunListAction {
    RunItem {
        mount: String,
        name: String,
    },
    StopBuilds {
        build_ids: Vec<QueryId>,
        mount: String,
        name: String,
    },
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum RunListRowData {
    RunItem {
        mount: String,
        name: String,
    },
    StopBuilds {
        build_ids: Vec<QueryId>,
        mount: String,
        name: String,
    },
    #[default]
    None,
}

impl ActionDefaultRef for RunListRowData {
    fn default_ref() -> &'static Self {
        &RunListRowData::None
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DesktopRunList {
    #[deref]
    view: View,
}

impl DesktopRunList {
    const ROW_HEIGHT: f64 = 34.0;

    fn empty_fill_rows(list: &PortalList, cx: &Cx2d, used_rows: usize) -> usize {
        let viewport_h = list.area().rect(cx).size.y.max(0.0);
        if viewport_h <= 0.0 {
            return 1usize.saturating_sub(used_rows);
        }
        let visible_rows = ((viewport_h / Self::ROW_HEIGHT).ceil() as usize).max(1);
        visible_rows.saturating_sub(used_rows)
    }

    fn draw_entries(&mut self, cx: &mut Cx2d, list: &mut PortalList, data: &AppData) {
        let Some(active_mount) = data.active_mount.as_deref() else {
            self.draw_empty(cx, list, "Select a mount");
            return;
        };

        let Some(entries) = data.mounts.get(active_mount).map(|mount| &mount.run_items) else {
            self.draw_empty(cx, list, "Loading run targets...");
            return;
        };
        let mut running_builds_by_package: std::collections::HashMap<&str, Vec<QueryId>> =
            std::collections::HashMap::new();
        for (build_id, mount) in &data.build_to_mount {
            if mount != active_mount {
                continue;
            }
            let Some(package) = data.build_package.get(build_id) else {
                continue;
            };
            running_builds_by_package
                .entry(package.as_str())
                .or_default()
                .push(*build_id);
        }
        for build_ids in running_builds_by_package.values_mut() {
            build_ids.sort_by_key(|build_id| build_id.0);
        }

        if entries.is_empty() {
            self.draw_empty(cx, list, "No run items available");
            return;
        }

        let empty_rows = Self::empty_fill_rows(list, cx, entries.len());
        let item_count = entries.len() + empty_rows;
        list.set_item_range(cx, 0, item_count);
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };
            let Some(entry) = entries.get(item_id) else {
                let mut item = list.item(cx, item_id, id!(Empty)).as_view();
                script_apply_eval!(cx, item, {
                    draw_bg +: {is_even: #(is_even_f)}
                });
                item.label(cx, ids!(info_label)).set_text(cx, "");
                item.draw_all(cx, &mut Scope::empty());
                continue;
            };

            let mut item = list.item(cx, item_id, id!(Item)).as_view();
            let is_running = running_builds_by_package.contains_key(entry.name.as_str());
            script_apply_eval!(cx, item, {
                draw_bg +: {
                    is_even: #(is_even_f)
                    is_running: #(if is_running {1.0} else {0.0})
                }
            });
            let button = item.button(cx, ids!(row_button));
            button.set_text(cx, &entry.name);
            item.view(cx, ids!(play_icon)).set_visible(cx, !is_running);
            item.view(cx, ids!(stop_icon)).set_visible(cx, is_running);
            item.view(cx, ids!(status_badge)).set_visible(cx, is_running);
            if let Some(build_ids) = running_builds_by_package.get(entry.name.as_str()) {
                button.set_action_data(RunListRowData::StopBuilds {
                    build_ids: build_ids.clone(),
                    mount: active_mount.to_string(),
                    name: entry.name.clone(),
                });
            } else {
                button.set_action_data(RunListRowData::RunItem {
                    mount: active_mount.to_string(),
                    name: entry.name.clone(),
                });
            }
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

impl Widget for DesktopRunList {
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
        let run_list = self.view.portal_list(cx, ids!(list));
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if !run_list.any_items_with_actions(actions) {
                return;
            }
            for (_item_id, item) in run_list.items_with_actions(actions) {
                let button = item.button(cx, ids!(row_button));
                if let Some(modifiers) = button.clicked_modifiers(actions) {
                    match button.action_data().cast_ref() {
                        RunListRowData::RunItem { mount, name } => {
                            cx.widget_action(
                                uid,
                                DesktopRunListAction::RunItem {
                                    mount: mount.clone(),
                                    name: name.clone(),
                                },
                            );
                        }
                        RunListRowData::StopBuilds {
                            build_ids,
                            mount,
                            name,
                        } => {
                            cx.widget_action(
                                uid,
                                DesktopRunListAction::StopBuilds {
                                    build_ids: build_ids.clone(),
                                    mount: mount.clone(),
                                    name: name.clone(),
                                },
                            );
                        }
                        RunListRowData::None => {}
                    }
                    let _ = modifiers;
                }
            }
        }
    }
}

impl DesktopRunListRef {
    pub fn requested_action(&self, actions: &Actions) -> Option<DesktopRunListAction> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            return Some(item.cast());
        }
        None
    }
}
