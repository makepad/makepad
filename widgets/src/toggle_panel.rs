use crate::{button::*, makepad_derive_widget::*, makepad_draw::*, view::*, widget::*};

live_design! {
    import crate::button::ButtonBase;
    import crate::view::ViewBase;
    //import makepad_widgets::theme_desktop_dark::*;
    import makepad_draw::shader::std::*;

    ICON_CLOSE_PANEL = dep("crate://self/resources/icons/close_left_panel.svg")
    ICON_OPEN_PANEL = dep("crate://self/resources/icons/open_left_panel.svg")

    // Copy of cached view from base, as can't be imported directly here to avoid infinite recursion.
    CachedView = <ViewBase> {
        optimize: Texture,
        draw_bg: {
            texture image: texture2d
            uniform marked: float,
            varying scale: vec2
            varying shift: vec2
            fn vertex(self) -> vec4 {
                let dpi = self.dpi_factor;
                let ceil_size = ceil(self.rect_size * dpi) / dpi
                let floor_pos = floor(self.rect_pos * dpi) / dpi
                self.scale = self.rect_size / ceil_size;
                self.shift = (self.rect_pos - floor_pos) / ceil_size;
                return self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
            }
            /*fn pixel(self) -> vec4 {
                return sample2d_rt(self.image, self.pos * self.scale + self.shift) + vec4(self.marked, 0.0, 0.0, 0.0);
            }*/
        }
    }

    FadeView = <CachedView> {
        draw_bg: {
            instance opacity: 1.0

            fn pixel(self) -> vec4 {
                let color = sample2d_rt(self.image, self.pos * self.scale + self.shift) + vec4(self.marked, 0.0, 0.0, 0.0);
                return Pal::premul(vec4(color.xyz, color.w * self.opacity))
            }
        }
    }

    CustomButton = <ButtonBase> {
        draw_bg: {
            instance hover: 0.0
            instance color: #0000
            instance color_hover: #fff
            instance border_width: 1.0
            instance border_color: #0000
            instance border_color_hover: #fff
            instance radius: 2.5

            fn get_color(self) -> vec4 {
                return mix(self.color, mix(self.color, self.color_hover, 0.2), self.hover)
            }

            fn get_border_color(self) -> vec4 {
                return mix(self.border_color, mix(self.border_color, self.border_color_hover, 0.2), self.hover)
            }

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_width,
                    self.border_width,
                    self.rect_size.x - (self.border_width * 2.0),
                    self.rect_size.y - (self.border_width * 2.0),
                    max(1.0, self.radius)
                )
                sdf.fill_keep(self.get_color())
                if self.border_width > 0.0 {
                    sdf.stroke(self.get_border_color(), self.border_width)
                }
                return sdf.result;
            }
        }

        draw_icon: {
            instance color: #fff
            instance color_hover: #000
            uniform rotation_angle: 0.0,

            fn get_color(self) -> vec4 {
                return mix(self.color, mix(self.color, self.color_hover, 0.2), self.hover)
            }

            // Support rotation of the icon
            fn clip_and_transform_vertex(self, rect_pos: vec2, rect_size: vec2) -> vec4 {
                let clipped: vec2 = clamp(
                    self.geom_pos * rect_size + rect_pos,
                    self.draw_clip.xy,
                    self.draw_clip.zw
                )
                self.pos = (clipped - rect_pos) / rect_size

                // Calculate the texture coordinates based on the rotation angle
                let angle_rad = self.rotation_angle * 3.14159265359 / 180.0;
                let cos_angle = cos(angle_rad);
                let sin_angle = sin(angle_rad);
                let rot_matrix = mat2(
                    cos_angle, -sin_angle,
                    sin_angle, cos_angle
                );
                self.tex_coord1 = mix(
                    self.icon_t1.xy,
                    self.icon_t2.xy,
                    (rot_matrix * (self.pos.xy - vec2(0.5))) + vec2(0.5)
                );

                return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                    clipped.x,
                    clipped.y,
                    self.draw_depth + self.draw_zbias,
                    1.
                )))
            }
        }
        icon_walk: {width: 14, height: 14}

        draw_text: {
            text_style: {font_size: 9},
            fn get_color(self) -> vec4 {
                return self.color;
            }
        }
    }

    ToggleButton = <CustomButton> {
        width: Fit,
        height: Fit,
        icon_walk: {width: 20, height: 20},
        draw_icon: {
            fn get_color(self) -> vec4 {
                return #475467;
            }
        }
    }

    TogglePanelBase = {{TogglePanel}} {
        flow: Overlay,
        width: 300,
        height: Fill,

        open_content = <FadeView> {
            width: Fill
            height: Fill
        }

        persistent_content = <ViewBase> {
            height: Fit
            width: Fill
            default = <ViewBase> {
                height: Fit,
                width: Fill,
                padding: {top: 58, left: 15, right: 15}
                spacing: 10,

                before = <ViewBase> {
                    height: Fit,
                    width: Fit,
                    spacing: 10,
                }

                close = <ToggleButton> {
                    draw_icon: {
                        svg_file: (ICON_CLOSE_PANEL),
                    }
                }

                open = <ToggleButton> {
                    visible: false,
                    draw_icon: {
                        svg_file: (ICON_OPEN_PANEL),
                    }
                }

                after = <ViewBase> {
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
