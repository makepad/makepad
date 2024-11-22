use crate::state::State;
use makepad_widgets::*;

live_design!(
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;

    BG_COLOR = #3

    Ui = {{Ui}} {
        flow: Right
        padding: { top: 30 }
        show_bg: true
        draw_bg: {
            fn pixel(self) -> vec4 {
                return (BG_COLOR);
            }
        }
        body = <View> { width: 0, height: 0 }
        img_list = <PortalList> {
            img_btn = <Button> {
                width: Fill
            }
        }
        img = <Image> {
            width: Fill
            height: Fill
            fit: Smallest
        }
    }
);

#[derive(Live, LiveHook, Widget)]
pub struct Ui {
    #[deref]
    deref: Window,
}

impl Widget for Ui {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.deref.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let state = scope.data.get::<State>().unwrap();
        let img = self.deref.image(id!(img));

        if !img.has_texture() && !state.images.is_empty() {
            img.load_image_file_by_path(cx, state.images[0].to_str().unwrap())
                .unwrap();
        }

        let filenames = state
            .images
            .iter()
            .map(|i| i.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();

        let range_end = state.images.len();
        while let Some(widget) = self.deref.draw_walk(cx, scope, walk).step() {
            if let Some(mut img_list) = widget.as_portal_list().borrow_mut() {
                img_list.set_item_range(cx, 0, range_end);

                while let Some(index) = img_list.next_visible_item(cx) {
                    if index < range_end {
                        let item = img_list.item(cx, index, live_id!(img_btn)).unwrap();
                        item.set_text(&filenames[index]);
                        item.draw_all(cx, scope);
                    }
                }
            }
        }

        DrawStep::done()
    }
}

impl WidgetMatchEvent for Ui {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let state = scope.data.get_mut::<State>().unwrap();
        let items_with_actions = self
            .deref
            .portal_list(id!(img_list))
            .items_with_actions(actions);

        let img_clicked = items_with_actions.iter().find(|(_index, widget)| {
            match widget.as_button().borrow_mut() {
                Some(btn) => btn.clicked(actions),
                None => false,
            }
        });

        if let Some((_index, widget)) = img_clicked {
            let img = self.deref.image(id!(img));
            img.load_image_file_by_path(
                cx,
                state.root().unwrap().join(widget.text()).to_str().unwrap(),
            )
            .unwrap();
            img.redraw(cx);
        }
    }
}
