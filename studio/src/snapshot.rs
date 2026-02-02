use {
    crate::{app::AppData, file_system::file_system::SnapshotImageData, makepad_widgets::*},
    makepad_platform::studio::{StudioScreenshotRequest, StudioToApp},
    std::env,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SnapshotBase = #(Snapshot::register_widget(vm))

    mod.widgets.SnapshotItem = RoundedView {
        height: Fit
        width: Fill
        draw_bg +: {color: #2}
        flow: Down
        align: Align{x: 0.5}

        $message: Label {text: "test" width: Fill height: Fit}
        $run_button: ButtonFlat {
            width: Fit
            height: Fit
            padding: theme.mspace_2
            margin: 0.
            text: ""
            icon_walk: Walk{
                width: 12
                height: Fit
                margin: Inset{ left: 10 }
            }

            draw_icon +: {
                color: theme.color_u_4
                svg_file: crate_resource("self://resources/icons/icon_run.svg")
            }
            icon_walk: Walk{ width: 9. }
        }
        $image: Image {
            width: 200
            height: 100
            margin: Inset{top: 10 bottom: 10}
            fit: Horizontal
            draw_bg +: {
                hover: instance(0.0)
                down: instance(0.0)
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, 4.0)
                    let max_scale = vec2(0.92)
                    let scale = vec2(1.0).mix(max_scale, self.hover)
                    let pan = vec2(0.0).mix((vec2(1.0) - max_scale) * 0.5, self.hover) * self.image_scale
                    let color = self.get_color_scale_pan(scale * self.image_scale, pan + self.image_pan) + vec4(0.0).mix(vec4(0.1), self.down)
                    if color.a < 0.0001 {
                        color = #3
                    }
                    sdf.fill_keep(color)
                    sdf.stroke(
                        #x0000.mix(#x0006, self.hover).mix(#xfff2, self.down),
                        1.0
                    )

                    return sdf.result
                }
            }
        }
    }

    mod.widgets.Snapshot = set_type_default() do mod.widgets.SnapshotBase {
        height: Fill
        width: Fill
        flow: Down
        RectShadowView {
            width: Fill
            height: Fit
            flow: Down
            align: Align{ x: 0. y: 0. }
            margin: Inset{ top: -1. }
            padding: theme.mspace_2
            spacing: 0.
            draw_bg +: {
                border_size: 0.0
                border_color: theme.color_bevel_outset_1
                shadow_color: theme.color_shadow
                shadow_radius: 5.0
                shadow_offset: vec2(0.0, 1.0)
                color: theme.color_fg_app
            }
            $content: View {
                height: Fit
                padding: Inset{top: 1}
                spacing: theme.space_2
                flow: Down
                View {
                    height: Fit
                    spacing: 5
                    $roots_dropdown: DropDownFlat { width: Fit }
                    $snapshot_button: ButtonFlat {text: "Snapshot"}
                    Filler {}
                    ToggleFlat {text: "Auto"}
                }
                $message_input: TextInputFlat {empty_text: "Description"}
            }
        }
        $list: PortalList {
            capture_overload: false
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            max_pull_down: 0
            height: Fill
            width: Fill
            flow: Down
            $SnapshotItem: mod.widgets.SnapshotItem {}
            $Empty: SolidView {
                cursor: MouseCursor.Default
                draw_bg +: {color: #44}
                width: Fill
                height: 80
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum SnapshotAction {
    Load(String),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Snapshot {
    #[deref]
    view: View,
    #[rust]
    request_id: u64,
}

impl Snapshot {
    fn draw_snapshots(
        &mut self,
        cx: &mut Cx2d,
        list: &mut PortalList,
        scope: &mut Scope,
        root_id: usize,
    ) {
        let data = scope.data.get_mut::<AppData>().unwrap();
        let file_system = &mut data.file_system;
        let git_log = file_system.git_logs.get(root_id as usize).unwrap();
        list.set_item_range(cx, 0, git_log.commits.len());
        while let Some(item_id) = list.next_visible_item(cx) {
            let item = if let Some(commit) = git_log.commits.get(item_id) {
                let item = list.item(cx, item_id, live_id!(SnapshotItem)).as_view();
                item.label(ids!(message)).set_text(cx, &commit.message);
                // lets construct a snapshot image filepath from the commit message
                // check if we have a image path or not
                let image = item.image(ids!(image));

                let load = match file_system.snapshot_image_data.borrow().get(&commit.hash) {
                    Some(SnapshotImageData::Loading) => {
                        image.set_visible(cx, true);
                        false
                    }
                    Some(SnapshotImageData::Error) => {
                        image.set_visible(cx, false);
                        false
                    }
                    Some(SnapshotImageData::Loaded { data, path }) => {
                        image.set_visible(cx, true);
                        image
                            .load_image_from_data_async(cx, &path, data.clone())
                            .ok();
                        false
                    }
                    None => true,
                };
                if load {
                    file_system
                        .file_client
                        .load_snapshot_image(&git_log.root, &commit.hash);
                }
                item
            } else {
                list.item(cx, item_id, live_id!(Empty)).as_view()
            };
            item.draw_all(cx, &mut Scope::empty());
        }
    }

    fn load_snapshot(&mut self, _cx: &mut Cx, data: &mut AppData, item_id: usize) {
        let root_id = self.drop_down(ids!(roots_dropdown)).selected_item();
        let git_log = data.file_system.git_logs.get(root_id).unwrap();
        if let Some(commit) = git_log.commits.get(item_id) {
            data.file_system
                .load_snapshot(git_log.root.clone(), commit.hash.clone());
        }
    }

    fn make_snapshot(&mut self, _cx: &mut Cx, data: &mut AppData) {
        let root_id = self.drop_down(ids!(roots_dropdown)).selected_item();
        let git_log = data.file_system.git_logs.get(root_id).unwrap();
        // we should find all active build ids with the same root

        let mut iter = data
            .build_manager
            .active
            .builds_with_root(git_log.root.clone());
        if let Some(item) = iter.next() {
            // we should do a shell git commit at the right path
            let message = self.view(ids!(message_input)).text();
            if message.len() == 0 {
                return;
            }
            data.file_system
                .create_snapshot(git_log.root.clone(), message);
            data.build_manager
                .active_build_websockets
                .lock()
                .unwrap()
                .borrow_mut()
                .send_studio_to_app(
                    *item.0,
                    StudioToApp::Screenshot(StudioScreenshotRequest {
                        kind_id: 0,
                        request_id: self.request_id,
                    }),
                );
            self.request_id += 1;
        }
    }
}

impl Widget for Snapshot {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let data = scope.data.get_mut::<AppData>().unwrap();

        let dd = self.drop_down(ids!(roots_dropdown));
        let mut i = data.file_system.git_logs.iter();
        dd.set_labels_with(cx, |label| {
            i.next().map(|m| label.push_str(&m.root));
        });
        let root_id = dd.selected_item();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_snapshots(cx, &mut *list, scope, root_id)
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let snapshots = self.view.portal_list(ids!(list));
        self.view.handle_event(cx, event, scope);
        let data = scope.data.get_mut::<AppData>().unwrap();
        if let Event::Actions(actions) = event {
            if self.view.button(ids!(snapshot_button)).clicked(actions) {
                self.make_snapshot(cx, data);
            }
            if let Some(_search) = self.view.text_input(ids!(search_input)).changed(&actions) {}
            for (item_id, _item) in snapshots.items_with_actions(&actions) {
                if let Some(wa) = actions.widget_action(ids!(run_button)) {
                    if wa.widget().as_button().pressed(actions) {
                        self.load_snapshot(cx, data, item_id);
                    }
                }
            }
        }
    }
}

impl SnapshotRef {
    pub fn set_message(&self, cx: &mut Cx, message: String) {
        if let Some(inner) = self.borrow_mut() {
            inner.view(ids!(message_input)).set_text(cx, &message);
        }
    }
}
