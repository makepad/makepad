use {
    crate::{
        app::{AppAction, AppData},
        build_manager::{build_manager::*, build_protocol::*},
        makepad_widgets::*,
    },
    std::env,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RunListBase = #(RunList::register_widget(vm))

    mod.widgets.BuildItem = mod.widgets.View {
        width: Fill
        height: Fit
        show_bg: true

        draw_bg +: {
            is_even: instance(0.0)
            active: instance(0.0)
            hover: instance(0.0)
            pixel: fn() {
                return theme.color_bg_even.mix(
                    theme.color_bg_odd,
                    self.is_even
                ).mix(
                    theme.color_outset_active,
                    self.active
                )
            }
        }
    }

    mod.widgets.RunButton = mod.widgets.CheckBox {
        width: Fill

        margin: theme.mspace_h_1
        draw_bg +: {
            size: uniform(3.5)
            length: uniform(3.0)
            width: uniform(1.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let left = 3.0
                let sz = self.size
                let c = vec2(left + sz, self.rect_size.y * 0.5 - 1.0)

                // PAUSE
                sdf.box(
                    sz * 0.5,
                    sz * 2.25,
                    sz * 0.9,
                    sz * 3.0,
                    1.0
                )

                sdf.box(
                    sz * 1.75,
                    sz * 2.25,
                    sz * 0.9,
                    sz * 3.0,
                    1.0
                )

                sdf.fill(theme.color_u_hidden.mix(theme.color_w.mix(theme.color_label_outer_hover, self.hover), self.active))

                // PLAY
                sdf.rotate(self.active * 0.5 * PI + 0.5 * PI, c.x, c.y)
                sdf.move_to(c.x - sz, c.y + sz)
                sdf.line_to(c.x, c.y - sz)
                sdf.line_to(c.x + sz, c.y + sz)
                sdf.close_path()
                sdf.fill(theme.color_u_4.mix(theme.color_label_outer_hover, self.hover).mix(theme.color_u_hidden, self.active))

                return sdf.result
            }
        }
    }

    mod.widgets.RunList = set_type_default() do mod.widgets.RunListBase {
        width: Fill
        height: Fill

        $list: FlatList {
            height: Fill
            width: Fill
            flow: Down
            grab_key_focus: true
            drag_scrolling: false

            $Target: mod.widgets.BuildItem {
                padding: 0
                $check: mod.widgets.RunButton { margin: Inset{left: 23.} }
            }

            $Binary: mod.widgets.BuildItem {
                flow: Right

                $fold: FoldButton {
                    height: 25
                    width: 15
                    margin: Inset{ left: theme.space_2 }
                    animator +: { active +: { default: @off } }
                    draw_bg +: {
                        size: uniform(3.75)
                        active: 0.0

                        pixel: fn() {
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            let left = 2.0
                            let sz = self.size
                            let c = vec2(left + sz, self.rect_size.y * 0.5)

                            // PLUS
                            sdf.box(0.5, sz * 3.0, sz * 2.5, sz * 0.7, 1.0)
                            // vertical
                            sdf.fill_keep(#6.mix(#8, self.hover))
                            sdf.box(sz * 1.0, sz * 2.125, sz * 0.7, sz * 2.5, 1.0)

                            sdf.fill_keep(#6.mix(#8, self.hover).mix(#fff0, self.active))

                            return sdf.result
                        }
                    }
                }
                $check: mod.widgets.RunButton {}
            }

            $Empty: mod.widgets.BuildItem {
                height: Fit
                width: Fill
                cursor: MouseCursor.Default
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum RunListAction {
    Create(LiveId, String),
    Destroy(LiveId),
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum ActionData {
    RunMain {
        binary_id: usize,
    },
    RunTarget {
        target: BuildTarget,
        binary_id: usize,
    },
    FoldBinary {
        binary_id: usize,
    },
    #[default]
    None,
}

impl ActionDefaultRef for ActionData {
    fn default_ref() -> &'static Self {
        &ActionData::None
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RunList {
    #[deref]
    view: View,
}

impl RunList {
    fn draw_run_list(
        &mut self,
        cx: &mut Cx2d,
        list: &mut FlatList,
        build_manager: &mut BuildManager,
    ) {
        let mut counter = 0u32;
        for (binary_id, binary) in build_manager.binaries.iter().enumerate() {
            let is_even = counter & 1 == 0;

            let item_id = LiveId::from_str(&binary.name);
            let mut item = list.item(cx, item_id, id!($Binary)).unwrap().as_view();
            let name = &binary.name;
            let is_even_f = if is_even { 1.0 } else { 0.0 };

            script_apply_eval!(cx, item, {
                draw_bg +: {is_even: #(is_even_f)}
            });
            
            item.fold_button(ids!($fold))
                .set_action_data(ActionData::FoldBinary { binary_id });

            let cb = item.check_box(ids!($check));
            cb.set_text(name);
            cb.set_active(cx, build_manager.active.any_binary_active(&binary.name));
            cb.set_action_data(ActionData::RunMain { binary_id });

            item.draw_all(cx, &mut Scope::empty());
            counter += 1;
            
            if binary.open > 0.001 {
                for i in 0..BuildTarget::len() {
                    let is_even = counter & 1 == 0;
                    let item_id = item_id.bytes_append(&i.to_be_bytes());
                    let mut item = list.item(cx, item_id, id!($Target)).unwrap().as_view();
                    let height = 25.0 * binary.open;
                    let is_even_f = if is_even { 1.0 } else { 0.0 };
                    let target_name = BuildTarget::from_id(i).name();
                    script_apply_eval!(cx, item, {
                        height: #(height)
                        draw_bg +: {is_even: #(is_even_f)}
                    });
                    let cb = item.check_box(ids!($check));
                    cb.set_text(target_name);
                    cb.set_active(cx, build_manager.active.item_id_active(item_id));

                    cb.set_action_data(ActionData::RunTarget {
                        target: BuildTarget::from_id(i),
                        binary_id,
                    });
                    item.draw_all(cx, &mut Scope::empty());
                    counter += 1;
                }
            }
        }
        while list.space_left(cx) > 0.0 {
            let is_even = counter & 1 == 0;
            let item_id = LiveId::from_str("empty").bytes_append(&counter.to_be_bytes());
            let mut item = list.item(cx, item_id, id!($Empty)).unwrap().as_view();
            let height = list.space_left(cx).min(20.0);
            let is_even_f = if is_even { 1.0 } else { 0.0 };
            script_apply_eval!(cx, item, {
                height: #(height)
                draw_bg +: {is_even: #(is_even_f)}
            });
            item.draw_all(cx, &mut Scope::empty());
            counter += 1;
        }
    }
}

impl WidgetMatchEvent for RunList {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let build_manager = &mut scope.data.get_mut::<AppData>().unwrap().build_manager;
        let run_list = self.view.flat_list(ids!($list));
        for (_item_id, item) in run_list.items_with_actions(&actions) {
            let fb = item.fold_button(ids!($fold));
            if let Some(v) = fb.animating(&actions) {
                if let ActionData::FoldBinary { binary_id } = fb.action_data().cast_ref() {
                    build_manager.binaries[*binary_id].open = v;
                    item.redraw(cx);
                }
            }

            let cb = item.check_box(ids!($check));
            if let Some(change) = cb.changed(&actions) {
                item.redraw(cx);
                match cb.action_data().cast_ref() {
                    ActionData::RunTarget { target, binary_id } => {
                        if change {
                            build_manager.start_active_build(cx, *binary_id, *target);
                        } else {
                            build_manager.stop_active_build(cx, *binary_id, *target);
                        }
                    }
                    ActionData::RunMain { binary_id } => {
                        for i in 0..if change { 1 } else { BuildTarget::len() } {
                            let target = BuildTarget::from_id(i);
                            if change {
                                build_manager.start_active_build(cx, *binary_id, target);
                            } else {
                                build_manager.stop_active_build(cx, *binary_id, target);
                            }
                            cx.action(AppAction::ClearLog);
                        }
                    }
                    _ => (),
                }
            }
        }
    }
}

impl Widget for RunList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_flat_list().borrow_mut() {
                self.draw_run_list(
                    cx,
                    &mut *list,
                    &mut scope.data.get_mut::<AppData>().unwrap().build_manager,
                )
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}

impl BuildManager {}
