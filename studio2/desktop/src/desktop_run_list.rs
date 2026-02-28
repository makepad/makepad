use crate::{app::AppData, makepad_widgets::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopRunListBase = #(DesktopRunList::register_widget(vm))

    mod.widgets.RunPlayIcon = View {
        width: 14.0
        height: 14.0
        margin: Inset {left: 3.0 right: 6.0 top: 0.0 bottom: 0.0}
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.move_to(3.0, 2.0)
                sdf.line_to(11.0, 7.0)
                sdf.line_to(3.0, 12.0)
                sdf.close_path()
                sdf.fill(#x7BD88F)
                return sdf.result
            }
        }
    }

    mod.widgets.RunListItem = View {
        width: Fill
        height: 34.0
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        spacing: theme.space_2
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        show_bg: true

        draw_bg +: {
            is_even: instance(0.0)
            selected: instance(0.0)
            hover: instance(0.0)
            pixel: fn() {
                return theme.color_bg_even.mix(
                    theme.color_bg_odd,
                    self.is_even
                ).mix(
                    #x2A3B28,
                    self.selected
                ).mix(
                    #x233526,
                    self.hover
                )
            }
        }

        animator: Animator {
            ignore_missing: true
            hover: {
                default: @off
                off: AnimatorState {
                    from: {all: Forward {duration: 0.08}}
                    apply: {
                        draw_bg: {hover: 0.0}
                    }
                }
                on: AnimatorState {
                    cursor: MouseCursor.Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                    }
                }
            }
            select: {
                default: @off
                off: AnimatorState {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0}
                    }
                }
                on: AnimatorState {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0}
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
                color_hover: #0000
                color_pressed: #0000
                border_color: #0000
            }
            draw_text.color: #xE9F0FF
        }
    }

    mod.widgets.RunListEmpty = View {
        width: Fill
        height: 36.0
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

        list := FlatList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: true
            Item := mod.widgets.RunListItem {
                icon := mod.widgets.RunPlayIcon {}
            }
            Empty := mod.widgets.RunListEmpty {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopRunListAction {
    RunPackage { mount: String, package: String },
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum RunListRowData {
    RunPackage {
        mount: String,
        package: String,
        index: usize,
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
    #[rust]
    selected_index: Option<usize>,
}

impl DesktopRunList {
    fn draw_entries(&mut self, cx: &mut Cx2d, list: &mut FlatList, data: &AppData) {
        let Some(active_mount) = data.active_mount.as_deref() else {
            self.draw_empty(cx, list, "Select a mount");
            return;
        };

        let Some(entries) = data.mount_runnable_builds.get(active_mount) else {
            self.draw_empty(cx, list, "Loading run targets...");
            return;
        };

        if entries.is_empty() {
            self.draw_empty(cx, list, "No runnable packages found");
            return;
        }

        for (index, entry) in entries.iter().enumerate() {
            let item_id = LiveId::from_str("run_item").bytes_append(&(index as u64).to_be_bytes());
            let Some(item) = list.item(cx, item_id, id!(Item)) else {
                continue;
            };
            let mut item = item.as_view();
            let is_even_f = if index & 1 == 0 { 0.0 } else { 1.0 };
            let selected_f = if self.selected_index == Some(index) { 1.0 } else { 0.0 };
            script_apply_eval!(cx, item, {
                draw_bg +: {
                    is_even: #(is_even_f),
                    selected: #(selected_f)
                }
            });
            let button = item.button(cx, ids!(row_button));
            button.set_text(cx, &entry.package);
            button.set_action_data(RunListRowData::RunPackage {
                mount: active_mount.to_string(),
                package: entry.package.clone(),
                index,
            });
            item.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_empty(&mut self, cx: &mut Cx2d, list: &mut FlatList, text: &str) {
        let Some(item) = list.item(cx, id!(run_empty), id!(Empty)) else {
            return;
        };
        let item = item.as_view();
        item.label(cx, ids!(info_label)).set_text(cx, text);
        item.draw_all(cx, &mut Scope::empty());
    }
}

impl Widget for DesktopRunList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_flat_list().borrow_mut() {
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
        let run_list = self.view.flat_list(cx, ids!(list));
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            for (_item_id, item) in run_list.items_with_actions(actions) {
                let button = item.button(cx, ids!(row_button));
                if button.clicked(actions) {
                    if let RunListRowData::RunPackage {
                        mount,
                        package,
                        index,
                    } = button.action_data().cast_ref()
                    {
                        self.selected_index = Some(*index);
                        cx.widget_action(
                            uid,
                            DesktopRunListAction::RunPackage {
                                mount: mount.clone(),
                                package: package.clone(),
                            },
                        );
                    }
                }
            }
        }
    }
}

impl DesktopRunListRef {
    pub fn run_requested(&self, actions: &Actions) -> Option<(String, String)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DesktopRunListAction::RunPackage { mount, package } = item.cast() {
                return Some((mount, package));
            }
        }
        None
    }
}
