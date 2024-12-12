use crate::{button::*, makepad_derive_widget::*, makepad_draw::*, view::*, widget::*};

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    use crate::view_ui::CachedView;
    use crate::view_ui::View;
    use crate::button::Button; 
    
    pub TogglePanelBase = {{TogglePanel}} {}
    pub TOGGLE_PANEL_CLOSE_ICON = dep("crate://self/resources/icons/close_left_panel.svg")
    pub TOGGLE_PANEL_OPEN_ICON = dep("crate://self/resources/icons/open_left_panel.svg")
    pub TogglePanel = <TogglePanelBase> {
        flow: Overlay,
        width: 300,
        height: Fill,
        
        open_content = <CachedView> {
            width: Fill
            height: Fill
            
            draw_bg: {
                instance opacity: 1.0
                    
                fn pixel(self) -> vec4 {
                    let color = sample2d_rt(self.image, self.pos * self.scale + self.shift) + vec4(self.marked, 0.0, 0.0, 0.0);
                    return Pal::premul(vec4(color.xyz, color.w * self.opacity))
                }
            }
        }
        
        persistent_content = <View> {
            height: Fit
            width: Fill
            default = <View> {
                height: Fit,
                width: Fill,
                padding: {top: 58, left: 15, right: 15}
                spacing: 10,
                
                before = <View> {
                    height: Fit,
                    width: Fit,
                    spacing: 10,
                }
                
                close = <Button> {
                    draw_icon: {
                        svg_file: (TOGGLE_PANEL_CLOSE_ICON),
                    }
                }
                
                open = <Button> {
                    visible: false,
                    draw_icon: {
                        svg_file: (TOGGLE_PANEL_OPEN_ICON),
                    }
                }
                
                after = <View> {
                    height: Fit,
                    width: Fit,
                    spacing: 10,
                }
            }
        }
        
        animator: {
            panel = {
                default: open,
                open = {
                    redraw: true,
                    from: {all: Forward {duration: 0.3}}
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    apply: {animator_panel_progress: 1.0, open_content = { draw_bg: {opacity: 1.0} }}
                }
                close = {
                    redraw: true,
                    from: {all: Forward {duration: 0.3}}
                    ease: ExpDecay {d1: 0.80, d2: 0.97}
                    apply: {animator_panel_progress: 0.0, open_content = { draw_bg: {opacity: 0.0} }}
                }
            }
        }
    }
}

/// A toggable side panel that can be expanded and collapsed to a maximum and minimum size.
#[derive(Live, Widget)]
pub struct TogglePanel {
    #[deref]
    view: View,

    /// Internal use only. Used by the animator to track the progress of the panel
    /// animation to overcome some limitations (for ex: `apply_over` doesn't work well
    /// over the animator).
    #[live]
    animator_panel_progress: f32,

    /// The size of the panel when it is fully open.
    #[live(300.0)]
    open_size: f32,

    /// The size of the panel when it is fully closed.
    #[live(110.0)]
    close_size: f32,

    #[animator]
    animator: Animator,
}

impl LiveHook for TogglePanel {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        if self.is_open(cx) {
            self.animator_panel_progress = 1.0;
        } else {
            self.animator_panel_progress = 0.0;
        };
    }
}

impl Widget for TogglePanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx)
        };

        if let Event::Actions(actions) = event {
            let open = self.button(id!(open));
            let close = self.button(id!(close));

            if open.clicked(actions) {
                open.set_visible(false);
                close.set_visible(true);
                self.set_open(cx, true);
            }

            if close.clicked(actions) {
                close.set_visible(false);
                open.set_visible(true);
                self.set_open(cx, false);
            }
        }

        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut walk = walk;

        let size_range = self.open_size - self.close_size;
        let size = self.close_size + size_range * self.animator_panel_progress;
        walk.width = Size::Fixed(size.into());

        self.view.draw_walk(cx, scope, walk)
    }
}

impl TogglePanel {
    /// Returns whether the panel is currently open.
    pub fn is_open(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, id!(panel.open))
    }

    /// Sets whether the panel is open. Causes the panel to animate to the new state.
    pub fn set_open(&mut self, cx: &mut Cx, open: bool) {
        if open {
            self.animator_play(cx, id!(panel.open));
        } else {
            self.animator_play(cx, id!(panel.close));
        }
    }
}

impl TogglePanelRef {
    /// Calls `is_open` on it's inner.
    pub fn is_open(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_open(cx)
        } else {
            false
        }
    }

    /// Calls `set_open` on it's inner.
    pub fn set_open(&self, cx: &mut Cx, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_open(cx, open);
        }
    }
}
