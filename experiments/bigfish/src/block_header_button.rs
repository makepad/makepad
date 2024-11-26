use crate::makepad_widgets::*;

live_design! {
    use makepad_widgets::theme_desktop_dark::*;
    use makepad_widgets::base::*;
    use makepad_draw::shader::std::*;

    BlockHeaderButton = {{BlockHeaderButton}} {


        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0, hover: 0.0}
                        draw_icon: {pressed: 0.0, hover: 0.0}
                        draw_text: {pressed: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_icon: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {pressed: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                pressed = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
        }

        width: Fill,
        height: Fit,
        margin: {left:0.0, right: 0.0, top:0.0, bottom: 0.0}
        align: {x: 0.5, y: 0.5}
        padding: {left: 0.0, top: 5.0, right: 0.0, bottom: 5.0}

        label_walk: {
            width: Fit,
            height: Fit
        }

        draw_text: {
            instance hover: 0.0
            instance pressed: 0.0
            text_style: <THEME_FONT_LABEL>{
                font_size: 11.0
            }
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #0,
                        #4,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }

        draw_icon: {
            instance hover: 0.0
            instance pressed: 0.0
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #9,
                        #c,
                        self.hover
                    ),
                    #9,
                    self.pressed
                )
            }
        }

        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: 3.0
            instance bodytop: #53
            instance bodybottom: #5c
            fn pixel(self) -> vec4 {

                //let body = mix(mix(self.bodytop, self.bodybottom, self.hover), #33, self.pressed);

                return vec4(0.,0.,0.,0.);
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, DefaultNone)]
pub enum BlockHeaderButtonAction {
    None,
    Clicked,
    Pressed,
    Released,
    Select { id: u64 },
    Move { id: u64, dx: f64, dy: f64 },
    RecordDragStart { id: u64 },
    RecordDragEnd { id: u64 },
}

#[derive(Live, LiveHook, Widget)]
pub struct BlockHeaderButton {
    #[animator]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_icon: DrawIcon,
    #[live]
    icon_walk: Walk,
    #[live]
    label_walk: Walk,
    #[walk]
    walk: Walk,

    #[layout]
    layout: Layout,

    #[live(true)]
    grab_key_focus: bool,

    #[live]
    pub text: RcStringMut,

    #[live]
    pub blockid: u64,
    #[rust]
    pub dragging: bool,

    #[rust]
    pub draglast: DVec2,
}

impl Widget for BlockHeaderButton {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerMove(fe) => {
                if self.dragging {
                    cx.widget_action(
                        uid,
                        &scope.path,
                        BlockHeaderButtonAction::Move {
                            id: self.blockid,
                            dx: fe.abs.x - self.draglast.x,
                            dy: fe.abs.y - self.draglast.y,
                        },
                    );
                    self.draglast = fe.abs;
                }
            }
            Hit::FingerDown(fe) => {
                if self.grab_key_focus {
                    cx.set_key_focus(self.draw_bg.area());
                }
                self.dragging = true;
                self.draglast = fe.abs;
                cx.widget_action(uid, &scope.path, BlockHeaderButtonAction::Pressed);
              
                cx.widget_action(
                    uid,
                    &scope.path,
                    BlockHeaderButtonAction::RecordDragStart { id: self.blockid },
                );
                self.animator_play(cx, id!(hover.pressed));
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerUp(fe) => {
                if self.dragging {
                    self.dragging = false;

                    cx.widget_action(
                        uid,
                        &scope.path,
                        BlockHeaderButtonAction::RecordDragEnd { id: self.blockid },
                    );
                }
                if fe.is_over {
                    cx.widget_action(uid, &scope.path, BlockHeaderButtonAction::Clicked);
                    if fe.device.has_hovers() {
                        self.animator_play(cx, id!(hover.on));
                    } else {
                        self.animator_play(cx, id!(hover.off));
                    }
                } else {
                    cx.widget_action(uid, &scope.path, BlockHeaderButtonAction::Released);
                    self.animator_play(cx, id!(hover.off));
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_text
            .draw_walk(cx, self.label_walk, Align::default(), self.text.as_ref());
        self.draw_icon.draw_walk(cx, self.icon_walk);
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, v: &str) {
        self.text.as_mut_empty().push_str(v);
    }
}

impl BlockHeaderButtonRef {
    #[allow(dead_code)]
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let BlockHeaderButtonAction::Clicked =
            actions.find_widget_action(self.widget_uid()).cast()
        {
            return true;
        }
        false
    }

    #[allow(dead_code)]
    pub fn pressed(&self, actions: &Actions) -> bool {
        if let BlockHeaderButtonAction::Pressed =
            actions.find_widget_action(self.widget_uid()).cast()
        {
            return true;
        }
        false
    }
}

impl BlockHeaderButtonSet {
    #[allow(dead_code)]
    pub fn clicked(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.clicked(actions))
    }
    #[allow(dead_code)]
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.iter().any(|v| v.pressed(actions))
    }
}
