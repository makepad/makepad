use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::{
            event::finger::TouchState,
            text::{
                geom::Point,
                layouter::{LaidoutText, SelectionRect},
                selection::{Cursor, CursorPosition, Selection},
            },
            *,
        },
        widget::*,
    },
    std::rc::Rc,
    unicode_segmentation::{GraphemeCursor, UnicodeSegmentation},
};

live_design! {
    link widgets;

    use link::theme::*;
    use makepad_draw::shader::std::*;

    pub TextInputBase = {{TextInput}} {}

    pub TextInputFlat = <TextInputBase> {
        width: Fill, height: Fit,
        padding: <THEME_MSPACE_1> { left: (THEME_SPACE_2), right: (THEME_SPACE_2) }
        margin: <THEME_MSPACE_V_1> {}
        flow: Right { wrap: true },
        is_password: false,
        is_read_only: false,
        empty_text: "Your text here",

        draw_bg: {
            instance hover: 0.0
            instance focus: 0.0
            instance down: 0.0
            instance disabled: 0.0
            instance empty: 0.0

            uniform border_radius: (THEME_CORNER_RADIUS)
            uniform border_size: (THEME_BEVELING)

            uniform gradient_border_horizontal: 0.0;
            uniform gradient_fill_horizontal: 0.0;

            uniform color_dither: 1.0

            color: (THEME_COLOR_INSET)
            uniform color_hover: (THEME_COLOR_INSET_HOVER)
            uniform color_focus: (THEME_COLOR_INSET_FOCUS)
            uniform color_down: (THEME_COLOR_INSET_DOWN)
            uniform color_empty: (THEME_COLOR_INSET_EMPTY)
            uniform color_disabled: (THEME_COLOR_INSET_DISABLED)

            uniform color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            uniform color_2_hover: (THEME_COLOR_INSET_2_HOVER)
            uniform color_2_focus: (THEME_COLOR_INSET_2_FOCUS)
            uniform color_2_down: (THEME_COLOR_INSET_2_DOWN)
            uniform color_2_empty: (THEME_COLOR_INSET_2_EMPTY)
            uniform color_2_disabled: (THEME_COLOR_INSET_2_DISABLED)

            uniform border_color: (THEME_COLOR_BEVEL)
            uniform border_color_hover: (THEME_COLOR_BEVEL_HOVER)
            uniform border_color_focus: (THEME_COLOR_BEVEL_FOCUS)
            uniform border_color_down: (THEME_COLOR_BEVEL_DOWN)
            uniform border_color_empty: (THEME_COLOR_BEVEL_EMPTY)
            uniform border_color_disabled: (THEME_COLOR_BEVEL_DISABLED)

            uniform border_color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            uniform border_color_2_hover: (THEME_COLOR_BEVEL_INSET_2_HOVER)
            uniform border_color_2_focus: (THEME_COLOR_BEVEL_INSET_2_FOCUS)
            uniform border_color_2_down: (THEME_COLOR_BEVEL_INSET_2_DOWN)
            uniform border_color_2_empty: (THEME_COLOR_BEVEL_INSET_2_EMPTY)
            uniform border_color_2_disabled: (THEME_COLOR_BEVEL_INSET_2_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let color_2 = self.color;
                let color_2_hover = self.color_hover;
                let color_2_focus = self.color_focus;
                let color_2_down = self.color_down;
                let color_2_empty = self.color_empty;
                let color_2_disabled = self.color_disabled;

                let border_color_2 = self.border_color;
                let border_color_2_hover = self.border_color_hover;
                let border_color_2_focus = self.border_color_focus;
                let border_color_2_down = self.border_color_down;
                let border_color_2_empty = self.border_color_empty;
                let border_color_2_disabled = self.border_color_disabled;

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2;
                    color_2_hover = self.color_2_hover;
                    color_2_focus = self.color_2_focus;
                    color_2_down = self.color_2_down;
                    color_2_empty = self.color_2_empty;
                    color_2_disabled = self.color_2_disabled;
                }

                if (self.border_color_2.x > -0.5) {
                    border_color_2 = self.border_color_2;
                    border_color_2_hover = self.border_color_2_hover;
                    border_color_2_focus = self.border_color_2_focus;
                    border_color_2_down = self.border_color_2_down;
                    border_color_2_empty = self.border_color_2_empty;
                    border_color_2_disabled = self.border_color_2_disabled;
                }

                let border_sz_uv = vec2(
                    self.border_size / self.rect_size.x,
                    self.border_size / self.rect_size.y
                )

                let scale_factor_border = vec2(
                    self.rect_size.x / self.rect_size.x,
                    self.rect_size.y / self.rect_size.y
                );

                let gradient_border = vec2(
                    self.pos.x * scale_factor_border.x + dither,
                    self.pos.y * scale_factor_border.y + dither
                )

                let sz_inner_px = vec2(
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.
                );

                let scale_factor_fill = vec2(
                    self.rect_size.x / sz_inner_px.x,
                    self.rect_size.y / sz_inner_px.y
                );

                let gradient_fill = vec2(
                    self.pos.x * scale_factor_fill.x - border_sz_uv.x * 2. + dither,
                    self.pos.y * scale_factor_fill.y - border_sz_uv.y * 2. + dither
                )

                let gradient_border_dir = gradient_border.y;
                if (self.gradient_border_horizontal > 0.5) {
                    gradient_border_dir = gradient_border.x;
                }

                let gradient_fill_dir = gradient_fill.y;
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = gradient_fill.x;
                }

                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.,
                    self.rect_size.y - self.border_size * 2.,
                    self.border_radius
                )

                sdf.fill_keep(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color, color_2, gradient_fill_dir),
                                    mix(self.color_empty, color_2_empty, gradient_fill_dir),
                                    self.empty
                                ),
                                mix(self.color_focus, color_2_focus, gradient_fill_dir),
                                self.focus
                            ),
                            mix(
                                mix(self.color_hover, color_2_hover, gradient_fill_dir),
                                mix(self.color_down, color_2_down, gradient_fill_dir),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.color_disabled, color_2_disabled, gradient_fill_dir),
                        self.disabled
                    )
                );

                sdf.stroke(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.border_color, border_color_2, gradient_border_dir),
                                    mix(self.border_color_empty, border_color_2_empty, gradient_border_dir),
                                    self.empty
                                ),
                                mix(self.border_color_focus, border_color_2_focus, gradient_border_dir),
                                self.focus
                            ),
                            mix(
                                mix(self.border_color_hover, border_color_2_hover, gradient_border_dir),
                                mix(self.border_color_down, border_color_2_down, gradient_border_dir),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.border_color_disabled, border_color_2_disabled, gradient_border_dir),
                        self.disabled
                    ),
                    self.border_size
                );

                return sdf.result
            }
        }

        draw_text: {
            instance hover: 0.0
            instance focus: 0.0
            instance down: 0.0
            instance empty: 0.0
            instance disabled: 0.0

            color: (THEME_COLOR_TEXT)
            uniform color_hover: (THEME_COLOR_TEXT_HOVER)
            uniform color_focus: (THEME_COLOR_TEXT_FOCUS)
            uniform color_down: (THEME_COLOR_TEXT_DOWN)
            uniform color_disabled: (THEME_COLOR_TEXT_DISABLED)
            uniform color_empty: (THEME_COLOR_TEXT_PLACEHOLDER)
            uniform color_empty_hover: (THEME_COLOR_TEXT_PLACEHOLDER_HOVER)
            uniform color_empty_focus: (THEME_COLOR_TEXT_FOCUS)

            text_style: <THEME_FONT_REGULAR> {
                line_spacing: (THEME_FONT_WDGT_LINE_SPACING),
                font_size: (THEME_FONT_SIZE_P)
            }

            fn get_color(self) -> vec4 {
                return
                    mix(
                        mix(
                            mix(
                                mix(
                                    self.color,
                                    mix(
                                        self.color_hover,
                                        self.color_down,
                                        self.down
                                    ),
                                    self.hover
                                ),
                                self.color_empty,
                                self.empty
                            ),
                            self.color_focus,
                            self.focus
                        ),
                        self.color_disabled,
                        self.disabled
                    )
            }
        }

        draw_selection: {
            instance hover: 0.0
            instance focus: 0.0
            instance down: 0.0
            instance empty: 0.0
            instance disabled: 0.0

            uniform color_dither: 1.0
            uniform border_radius: (THEME_TEXTSELECTION_CORNER_RADIUS)
            uniform gradient_fill_horizontal: 0.0

            uniform color: (THEME_COLOR_SELECTION)
            uniform color_hover: (THEME_COLOR_SELECTION_HOVER)
            uniform color_focus: (THEME_COLOR_SELECTION_FOCUS)
            uniform color_down: (THEME_COLOR_SELECTION_DOWN)
            uniform color_empty: (THEME_COLOR_SELECTION_EMPTY)
            uniform color_disabled: (THEME_COLOR_SELECTION_DISABLED)

            uniform color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            uniform color_2_hover: (THEME_COLOR_SELECTION_HOVER)
            uniform color_2_focus: (THEME_COLOR_SELECTION_FOCUS)
            uniform color_2_down: (THEME_COLOR_SELECTION_DOWN)
            uniform color_2_empty: (THEME_COLOR_SELECTION_EMPTY)
            uniform color_2_disabled: (THEME_COLOR_SELECTION_DISABLED)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);

                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                let color_2 = self.color;
                let color_2_hover = self.color_hover;
                let color_2_focus = self.color_focus;
                let color_2_down = self.color_down;
                let color_2_empty = self.color_empty;
                let color_2_disabled = self.color_disabled;

                if (self.color_2.x > -0.5) {
                    color_2 = self.color_2;
                    color_2_hover = self.color_2_hover;
                    color_2_focus = self.color_2_focus;
                    color_2_down = self.color_2_down;
                    color_2_empty = self.color_2_empty;
                    color_2_disabled = self.color_2_disabled;
                }

                let gradient_fill_dir = self.pos.y + dither;
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither;
                }

                sdf.box(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                )

                sdf.fill(
                    mix(
                        mix(
                            mix(
                                mix(
                                    mix(self.color, color_2, gradient_fill_dir),
                                    mix(self.color_empty, color_2_empty, gradient_fill_dir),
                                    self.empty
                                ),
                                mix(self.color_focus, color_2_focus, gradient_fill_dir),
                                self.focus
                            ),
                            mix(
                                mix(self.color_hover, color_2_hover, gradient_fill_dir),
                                mix(self.color_down, color_2_down, gradient_fill_dir),
                                self.down
                            ),
                            self.hover
                        ),
                        mix(self.color_disabled, color_2_disabled, gradient_fill_dir),
                        self.disabled
                    )
                );
                return sdf.result;
            }
        }

        draw_cursor: {
            instance focus: 0.0
            instance down: 0.0
            instance empty: 0.0
            instance disabled: 0.0
            instance blink: 0.0

            uniform border_radius: 0.5

            uniform color: (THEME_COLOR_TEXT_CURSOR)

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.box(
                    0.0,
                    0.0,
                    self.rect_size.x,
                    self.rect_size.y,
                    self.border_radius
                );
                sdf.fill(
                    mix(THEME_COLOR_U_HIDDEN, self.color, (1.0-self.blink) * self.focus)
                );
                return sdf.result;
            }
        }

        draw_composition_underline: {
            uniform color: #8

            fn pixel(self) -> vec4 {
                return self.color;
            }
        }

        animator: {
            empty = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {empty: 0.0}
                        draw_text: {empty: 0.0}
                        draw_selection: {empty: 0.0}
                        draw_cursor: {empty: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {empty: 1.0}
                        draw_text: {empty: 1.0}
                        draw_selection: {empty: 1.0}
                        draw_cursor: {empty: 1.0}
                    }
                }
            }
            blink = {
                default: off
                off = {
                    from: {all: Forward {duration:0.05}}
                    apply: {
                        draw_cursor: {blink:0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.05}}
                    apply: {
                        draw_cursor: {blink:1.0}
                    }
                }
            }
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            disabled = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_selection: {disabled: 0.0}
                        draw_cursor: {disabled: 0.0}
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_selection: {disabled: 1.0}
                        draw_cursor: {disabled: 1.0}
                    }
                }
            }
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {down: 0.0, hover: 0.0}
                        draw_text: {down: 0.0, hover: 0.0}
                    }
                }

                on = {
                    from: {
                        all: Forward {duration: 0.1}
                        down: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                        draw_text: {down: 0.0, hover: [{time: 0.0, value: 1.0}],}
                    }
                }

                down = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                        draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0,}
                    }
                }
            }
            focus = {
                default: off
                off = {
                    from: {
                        all: Forward { duration: 0.25 }
                    }
                    apply: {
                        draw_bg: { focus: 0.0 }
                        draw_text: { focus: 0.0 },
                        draw_cursor: { focus: 0.0 },
                        draw_selection: { focus: 0.0 }
                    }
                }
                on = {
                    from: { all: Snap }
                    apply: {
                        draw_bg: { focus: 1.0 }
                        draw_text: { focus: 1.0 }
                        draw_cursor: { focus: 1.0 },
                        draw_selection: { focus: 1.0 }
                    }
                }
            }
        }
    }

    pub TextInput = <TextInputFlat> {
        draw_bg: {
            border_color: (THEME_COLOR_BEVEL_INSET_1)
            border_color_hover: (THEME_COLOR_BEVEL_INSET_1_HOVER)
            border_color_focus: (THEME_COLOR_BEVEL_INSET_1_FOCUS)
            border_color_down: (THEME_COLOR_BEVEL_INSET_1_DOWN)
            border_color_empty: (THEME_COLOR_BEVEL_INSET_1_EMPTY)
            border_color_disabled: (THEME_COLOR_BEVEL_INSET_1_DISABLED)

            border_color_2: (THEME_COLOR_BEVEL_INSET_1)
        }
    }

    pub TextInputGradientX = <TextInput> {
        draw_bg: {
            gradient_border_horizontal: 1.0;
            gradient_fill_horizontal: 1.0;

            color: (THEME_COLOR_INSET_1)
            color_hover: (THEME_COLOR_INSET_1_HOVER)
            color_focus: (THEME_COLOR_INSET_1_FOCUS)
            color_down: (THEME_COLOR_INSET_1_DOWN)
            color_empty: (THEME_COLOR_INSET_1_EMPTY)
            color_disabled: (THEME_COLOR_INSET_1_DISABLED)

            color_2: (THEME_COLOR_INSET_2)
        }

        draw_selection: {
            gradient_fill_horizontal: 1.0;

            color: (THEME_COLOR_SELECTION)
            color_hover: (THEME_COLOR_SELECTION_HOVER)
            color_focus: (THEME_COLOR_SELECTION_FOCUS)
            color_down: (THEME_COLOR_SELECTION_DOWN)
            color_empty: (THEME_COLOR_SELECTION_EMPTY)
            color_disabled: (THEME_COLOR_SELECTION_DISABLED)

            color_2: (THEME_COLOR_SELECTION)
            color_2_hover: (THEME_COLOR_SELECTION_HOVER)
            color_2_focus: (THEME_COLOR_SELECTION_FOCUS)
            color_2_down: (THEME_COLOR_SELECTION_DOWN)
            color_2_empty: (THEME_COLOR_SELECTION_EMPTY)
            color_2_disabled: (THEME_COLOR_SELECTION_DISABLED)
        }
    }


    pub TextInputGradientY = <TextInputGradientX> {
        draw_bg: {
            gradient_border_horizontal: 0.0;
            gradient_fill_horizontal: 0.0;
        }

        draw_selection: {
            gradient_fill_horizontal: 0.0;
        }
    }
}

#[derive(Live, Widget)]
pub struct TextInput {
    #[animator]
    animator: Animator,

    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_selection: DrawQuad,
    #[live]
    draw_cursor: DrawQuad,
    /// The quad used to draw a thin underline beneath text that is currently being composed
    /// via IME
    #[live]
    draw_composition_underline: DrawQuad,

    #[layout]
    layout: Layout,
    #[walk]
    walk: Walk,
    #[live]
    label_align: Align,

    #[live]
    is_password: bool,
    #[live]
    is_read_only: bool,
    /// Input mode controls both the mobile soft keyboard layout and widget-level
    /// input filtering. Ascii, Numeric, Decimal, and Tel modes filter input on
    /// all platforms. Url, Email, and Search only affect the keyboard layout on mobile.
    #[live]
    input_mode: InputMode,
    /// Autocapitalization hint for mobile soft keyboards. This only affects the
    /// keyboard's default shift state on iOS/Android — it does not transform input
    /// text and has no effect on desktop platforms.
    #[live]
    autocapitalize: AutoCapitalize,
    /// Autocorrection hint for mobile soft keyboards. Only affects iOS/Android;
    /// has no effect on desktop platforms.
    #[live]
    autocorrect: AutoCorrect,
    /// Return key appearance on mobile soft keyboards. On desktop, Enter/Return
    /// behavior is controlled by is_multiline instead.
    #[live]
    return_key_type: ReturnKeyType,
    /// Whether the text input is multiline.
    #[live(true)]
    is_multiline: bool,
    #[live]
    scroll_y: f64,
    #[live]
    empty_text: String,
    #[rust]
    text: String,
    #[live(0.5)]
    blink_speed: f64,

    #[rust]
    password_text: String,
    #[rust]
    laidout_text: Option<Rc<LaidoutText>>,
    #[rust]
    text_area: Area,
    #[rust]
    selection: Selection,
    #[rust]
    history: History,
    #[rust]
    blink_timer: Timer,
    /// Stores the cursor position from a tap when the tap landed on an existing selection.
    /// Defers collapsing the selection until FingerUp to distinguish tap from drag:
    /// - Tap (no drag): cursor moves to tap position, collapsing the selection
    /// - Drag: cleared, starts a new drag-to-select from that point instead
    #[rust]
    preserved_selection_cursor: Option<Cursor>,
    /// Skip finger move after long press to prevent selection changes
    #[rust]
    ignore_next_move: bool,

    // ===== IME (Input Method Editor) State =====
    //
    // For platform-level IME architecture, see `platform/src/ime.rs`.
    //
    // IME allows users to input complex characters (e.g., Chinese, Japanese, Korean)
    // through a composition process where text is previewed before being committed.
    // Similarly the composition process is used for autocorrect and autocompletion features, among others.
    //
    // ## Widget Sync Model
    // This widget is the source of truth for text content. The platform IME receives
    // our state via `sync_ime_state()` and sends changes back via TextInput events.
    // During active composition, the platform IME is temporarily authoritative.
    //
    // ## Echo Prevention (Two Mechanisms)
    // 1. `ime_update_frame` - Same-frame guard: skips sync entirely when IME just sent input
    //    (catches composition-end edge case where state changed but shouldn't echo)
    // 2. `last_sent_ime_*` - State-diff guard: only syncs when state actually differs
    //
    // ## Platform Differences in Event Handling
    // - Android: `full_state_sync` - receives complete text + selection + composition
    // - iOS: `replace_range` - receives specific range replacement for autocorrect/paste
    // - Both: `replace_last` + `input` - universal composition preview handling
    /// Byte index in self.text where the active IME composition starts.
    /// Only valid when has_composition() returns true.
    #[rust]
    composition_start: usize,
    /// Byte index in self.text where the active IME composition ends.
    /// When composition_end == composition_start, there is no active composition.
    #[rust]
    composition_end: usize,
    /// Frame ID when IME input was last received.
    ///
    /// SAME-FRAME ECHO PREVENTION:
    /// When the platform IME sends text input, we update self.text. During the draw
    /// phase of the same frame, update_ime_context() would normally sync our state
    /// back to the platform. But echoing back state the IME just sent us can confuse
    /// some IMEs (especially on Android where the InputConnection expects to be
    /// authoritative during composition).
    ///
    /// This works alongside `last_sent_ime_*` (state-diff guard) but catches cases
    /// where composition just ended: `has_composition()` is now false, state differs
    /// from last sent, but we still shouldn't echo because the IME just told us.
    #[rust]
    ime_update_frame: u64,
    /// Cached copy of the last text we sent to the platform IME.
    /// Used to prevent syncing back to the platform IME when the state hasn't changed from what we last sent.
    ///
    /// Without this, the following loop can occur:
    /// 1. Platform IME sends us text "abc"
    /// 2. We update self.text = "abc"
    /// 3. On next draw, we call sync_ime_state("abc")
    /// 4. Platform receives "abc", thinks it's new input
    /// 5. Platform sends "abc" back to us as a change event
    /// 6. Loop continues...
    #[rust]
    last_sent_ime_text: String,
    /// Cached selection start (byte index) we last sent to the platform IME.
    #[rust]
    last_sent_ime_sel_start: usize,
    /// Cached selection end (byte index) we last sent to the platform IME.
    #[rust]
    last_sent_ime_sel_end: usize,

    #[rust]
    last_layout_width: f64,
}

impl LiveHook for TextInput {
    fn apply_value_unknown(
        &mut self,
        cx: &mut Cx,
        apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> usize {
        if nodes[index].id == live_id!(text) {
            if !apply.from.is_update_from_doc() {
                return self.text.apply(cx, apply, index, nodes);
            }
        } else {
            cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
        }
        nodes.skip_node(index)
    }
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.check_text_is_empty(cx);
    }
}

impl TextInput {
    pub fn is_password(&self) -> bool {
        self.is_password
    }

    pub fn set_is_password(&mut self, cx: &mut Cx, is_password: bool) {
        self.is_password = is_password;
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
    }

    pub fn toggle_is_password(&mut self, cx: &mut Cx) {
        self.set_is_password(cx, !self.is_password);
    }

    pub fn is_read_only(&self) -> bool {
        self.is_read_only
    }

    pub fn set_is_read_only(&mut self, cx: &mut Cx, is_read_only: bool) {
        self.is_read_only = is_read_only;
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
    }

    pub fn toggle_is_read_only(&mut self, cx: &mut Cx) {
        self.set_is_read_only(cx, !self.is_read_only);
    }

    /// Build configuration for the platform soft keyboard from widget properties.
    ///
    /// This configuration controls the keyboard's appearance and behavior on mobile platforms
    /// (iOS/Android), including: keyboard layout (numeric, email, etc.), autocapitalization,
    /// autocorrection, and the return key type (Done, Go, Search, etc.). On desktop platforms,
    /// these settings have no effect.
    pub fn get_ime_config(&self) -> TextInputConfig {
        TextInputConfig {
            soft_keyboard: SoftKeyboardConfig {
                input_mode: self.input_mode,
                autocapitalize: self.autocapitalize,
                autocorrect: self.autocorrect,
                return_key_type: self.return_key_type,
            },
            is_multiline: self.is_multiline,
            is_secure: self.is_password,
        }
    }

    pub fn empty_text(&self) -> &str {
        &self.empty_text
    }

    pub fn set_empty_text(&mut self, cx: &mut Cx, empty_text: String) {
        self.empty_text = empty_text;
        if self.text.is_empty() {
            self.draw_bg.redraw(cx);
        }
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn set_selection(&mut self, cx: &mut Cx, selection: Selection) {
        self.selection = selection;
        self.history.force_new_edit_group();
        self.draw_bg.redraw(cx);
    }

    pub fn cursor(&self) -> Cursor {
        self.selection.cursor
    }

    pub fn set_cursor(&mut self, cx: &mut Cx, cursor: Cursor, keep_selection: bool) {
        self.set_selection(
            cx,
            Selection {
                anchor: if keep_selection {
                    self.selection.anchor
                } else {
                    cursor
                },
                cursor,
            },
        );
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.start().index..self.selection.end().index]
    }

    /// Returns true if there is an active IME composition in progress
    fn has_composition(&self) -> bool {
        self.composition_end > self.composition_start
    }

    /// Updates the IME text context for platform IME.
    ///
    /// ECHO PREVENTION:
    /// Only sends state to platform if it differs from what we last sent.
    fn update_ime_context(&mut self, cx: &mut Cx) {
        // Don't sync back to platform during active composition since the platform IME is the source of truth during it.
        if self.has_composition() {
            return;
        }

        use crate::makepad_platform::event::keyboard::CharOffset;

        // Convert byte indices to character offsets
        let sel_start_chars = self.text[..self.selection.start().index].chars().count();
        let sel_end_chars = self.text[..self.selection.end().index].chars().count();

        // Only send if state actually changed from what we last sent
        // This prevents the sync loop where IME sends state → we echo it back → IME gets confused
        if self.text != self.last_sent_ime_text
            || self.selection.start().index != self.last_sent_ime_sel_start
            || self.selection.end().index != self.last_sent_ime_sel_end
        {
            self.last_sent_ime_text = self.text.clone();
            self.last_sent_ime_sel_start = self.selection.start().index;
            self.last_sent_ime_sel_end = self.selection.end().index;

            // Sync via unified operation
            cx.sync_ime_state(
                self.text.clone(),
                CharOffset(sel_start_chars)..CharOffset(sel_end_chars),
                None, // Composition not tracked yet for outgoing sync
            );
        }
    }

    pub fn reset_blink_timer(&mut self, cx: &mut Cx) {
        self.animator_cut(cx, ids!(blink.off));
        if !self.is_read_only {
            cx.stop_timer(self.blink_timer);
            self.blink_timer = cx.start_timeout(self.blink_speed)
        }
    }

    fn cursor_to_position(&self, cursor: Cursor) -> Result<CursorPosition, ()> {
        let Some(laidout_text) = self.laidout_text.as_ref() else {
            return Err(());
        };
        let position = laidout_text.cursor_to_position(self.cursor_to_password_cursor(cursor));
        Ok(CursorPosition {
            row_index: position.row_index,
            x_in_lpxs: position.x_in_lpxs * self.draw_text.font_scale,
        })
    }

    fn point_in_lpxs_to_cursor(&self, point_in_lpxs: Point<f32>) -> Result<Cursor, ()> {
        let Some(laidout_text) = self.laidout_text.as_ref() else {
            return Err(());
        };
        let cursor =
            laidout_text.point_in_lpxs_to_cursor(point_in_lpxs / self.draw_text.font_scale);
        Ok(self.password_cursor_to_cursor(cursor))
    }

    fn position_to_cursor(&self, position: CursorPosition) -> Result<Cursor, ()> {
        let Some(laidout_text) = self.laidout_text.as_ref() else {
            return Err(());
        };
        let cursor = laidout_text.position_to_cursor(CursorPosition {
            row_index: position.row_index,
            x_in_lpxs: position.x_in_lpxs / self.draw_text.font_scale,
        });
        Ok(self.password_cursor_to_cursor(cursor))
    }

    fn selection_to_password_selection(&self, selection: Selection) -> Selection {
        Selection {
            cursor: self.cursor_to_password_cursor(selection.cursor),
            anchor: self.cursor_to_password_cursor(selection.anchor),
        }
    }

    fn cursor_to_password_cursor(&self, cursor: Cursor) -> Cursor {
        Cursor {
            index: self.index_to_password_index(cursor.index),
            prefer_next_row: cursor.prefer_next_row,
        }
    }

    fn password_cursor_to_cursor(&self, password_cursor: Cursor) -> Cursor {
        Cursor {
            index: self.password_index_to_index(password_cursor.index),
            prefer_next_row: password_cursor.prefer_next_row,
        }
    }

    fn index_to_password_index(&self, index: usize) -> usize {
        if !self.is_password {
            return index;
        }
        let grapheme_index = self.text[..index].graphemes(true).count();
        self.password_text
            .grapheme_indices(true)
            .nth(grapheme_index)
            .map_or(self.password_text.len(), |(index, _)| index)
    }

    fn password_index_to_index(&self, password_index: usize) -> usize {
        if !self.is_password {
            return password_index;
        }
        let grapheme_index = self.password_text[..password_index].graphemes(true).count();
        self.text
            .grapheme_indices(true)
            .nth(grapheme_index)
            .map_or(self.text.len(), |(index, _)| index)
    }

    fn inner_walk(&self) -> Walk {
        if self.walk.width.is_fit() {
            Walk::fit()
        } else {
            Walk::fill_fit()
        }
    }

    fn layout_text(&mut self, cx: &mut Cx2d) {
        let max_width = cx.turtle().inner_width();

        let width_changed = if self.last_layout_width.is_nan() {
            !max_width.is_nan()
        } else if max_width.is_nan() {
            true
        } else {
            self.last_layout_width != max_width
        };
        if width_changed {
            self.laidout_text = None;
        }

        if self.laidout_text.is_some() {
            return;
        }

        self.last_layout_width = max_width;

        let text = if self.is_password {
            self.password_text.clear();
            for grapheme in self.text.graphemes(true) {
                self.password_text
                    .push(if grapheme == "\n" { '\n' } else { '•' });
            }
            &self.password_text
        } else {
            &self.text
        };
        let turtle_rect = cx.turtle().inner_rect();
        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = cx.turtle().layout().flow == Flow::right_wrap();
        self.laidout_text = Some(self.draw_text.layout(
            cx,
            0.0,
            0.0,
            max_width_in_lpxs,
            wrap,
            self.label_align,
            text,
        ));
    }

    fn draw_text(&mut self, cx: &mut Cx2d) -> Rect {
        let inner_walk = self.inner_walk();
        let text_rect = if self.text.is_empty() {
            self.draw_text
                .draw_walk(cx, inner_walk, self.label_align, &self.empty_text)
        } else {
            let laidout_text = self.laidout_text.as_ref().unwrap();
            self.draw_text
                .draw_walk_laidout(cx, inner_walk, laidout_text)
        };
        cx.add_aligned_rect_area(&mut self.text_area, text_rect);
        text_rect
    }

    fn draw_cursor(&mut self, cx: &mut Cx2d, text_rect: Rect) -> Rect {
        let CursorPosition {
            row_index,
            x_in_lpxs,
        } = self
            .cursor_to_position(self.selection.cursor)
            .ok()
            .expect("layout should not be `None` because we called `layout_text` in `draw_walk`");
        let x_in_lpxs = x_in_lpxs.min(cx.turtle().inner_rect().size.x as f32 - 2.0);
        let laidout_text = self
            .laidout_text
            .as_ref()
            .expect("layout should not be `None` because we called `layout_text` in `draw_walk`");
        let row = &laidout_text.rows[row_index];
        let cursor_rect = rect(
            (x_in_lpxs - 1.0 * self.draw_text.font_scale) as f64,
            ((row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.draw_text.font_scale) as f64,
            (2.0 * self.draw_text.font_scale) as f64,
            ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.draw_text.font_scale) as f64,
        );
        self.draw_cursor
            .draw_abs(cx, cursor_rect.translate(text_rect.pos));
        cursor_rect
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, text_rect: Rect) {
        let laidout_text = self
            .laidout_text
            .as_ref()
            .expect("layout should not be `None` because we called `layout_text` in `draw_walk`");

        self.draw_selection.begin_many_instances(cx);
        for SelectionRect { rect_in_lpxs, .. } in
            laidout_text.selection_rects(self.selection_to_password_selection(self.selection))
        {
            self.draw_selection.draw_abs(
                cx,
                rect(
                    text_rect.pos.x + (rect_in_lpxs.origin.x * self.draw_text.font_scale) as f64,
                    text_rect.pos.y + (rect_in_lpxs.origin.y * self.draw_text.font_scale) as f64,
                    (rect_in_lpxs.size.width * self.draw_text.font_scale) as f64,
                    (rect_in_lpxs.size.height * self.draw_text.font_scale) as f64,
                ),
            );
        }
        self.draw_selection.end_many_instances(cx);
    }

    /// Draws a thin underline beneath the active IME composition range to visually indicate
    /// text that is still being composed and has not yet been committed.
    fn draw_composition_underline(&mut self, cx: &mut Cx2d, text_rect: Rect) {
        if !self.has_composition() {
            return;
        }

        let laidout_text = self
            .laidout_text
            .as_ref()
            .expect("layout should never be `None` here");

        let composition_selection = Selection {
            anchor: Cursor {
                index: self.composition_start.min(self.text.len()),
                prefer_next_row: false,
            },
            cursor: Cursor {
                index: self.composition_end.min(self.text.len()),
                prefer_next_row: false,
            },
        };

        let selection = self.selection_to_password_selection(composition_selection);
        let underline_height = 1.5 * self.draw_text.font_scale;

        self.draw_composition_underline.begin_many_instances(cx);
        for SelectionRect { rect_in_lpxs, .. } in laidout_text.selection_rects(selection) {
            let scaled_x =
                text_rect.pos.x + (rect_in_lpxs.origin.x * self.draw_text.font_scale) as f64;
            let scaled_y = text_rect.pos.y
                + ((rect_in_lpxs.origin.y + rect_in_lpxs.size.height) * self.draw_text.font_scale)
                    as f64
                - underline_height as f64;
            let scaled_w = (rect_in_lpxs.size.width * self.draw_text.font_scale) as f64;

            self.draw_composition_underline.draw_abs(
                cx,
                rect(scaled_x, scaled_y, scaled_w, underline_height as f64),
            );
        }
        self.draw_composition_underline.end_many_instances(cx);
    }

    /// Calculate the bounding rectangle of the current text selection in screen coordinates
    /// We use the draw_bg area which should give us the actual drawn position
    fn get_selection_rect(&self, cx: &Cx) -> Rect {
        let widget_rect = self.draw_bg.area().rect(cx);

        // If no layout yet, return a small rect below the widget
        let Some(laidout_text) = self.laidout_text.as_ref() else {
            return rect(
                widget_rect.pos.x,
                widget_rect.pos.y + widget_rect.size.y,
                10.0,
                20.0,
            );
        };

        // Get all selection rectangles
        let selection_rects =
            laidout_text.selection_rects(self.selection_to_password_selection(self.selection));

        if selection_rects.is_empty() {
            // No selection, return position below the widget
            return rect(
                widget_rect.pos.x,
                widget_rect.pos.y + widget_rect.size.y,
                10.0,
                20.0,
            );
        }

        // Calculate bounding box of all selection rects
        let first = &selection_rects[0].rect_in_lpxs;
        let mut min_x = first.origin.x;
        let mut min_y = first.origin.y;
        let mut max_x = first.origin.x + first.size.width;
        let mut max_y = first.origin.y + first.size.height;

        for SelectionRect { rect_in_lpxs, .. } in selection_rects.iter().skip(1) {
            min_x = min_x.min(rect_in_lpxs.origin.x);
            min_y = min_y.min(rect_in_lpxs.origin.y);
            max_x = max_x.max(rect_in_lpxs.origin.x + rect_in_lpxs.size.width);
            max_y = max_y.max(rect_in_lpxs.origin.y + rect_in_lpxs.size.height);
        }

        // Convert to screen coordinates using widget position as base
        let text_offset_x = widget_rect.pos.x + self.layout.padding.left as f64;
        let text_offset_y = widget_rect.pos.y + self.layout.padding.top as f64;

        let sel_x = text_offset_x + (min_x * self.draw_text.font_scale) as f64;
        let sel_y = text_offset_y + (min_y * self.draw_text.font_scale) as f64;
        let sel_width = ((max_x - min_x) * self.draw_text.font_scale) as f64;
        let sel_height = ((max_y - min_y) * self.draw_text.font_scale) as f64;

        rect(sel_x, sel_y, sel_width.max(10.0), sel_height.max(20.0))
    }

    fn scroll_to_cursor(&mut self, cx: &mut Cx2d) {
        // Compute the final size of the turtle, and obtain its inner height.
        cx.compute_final_size();
        let height = cx.turtle().inner_rect().size.y;

        // Compute the min and max y of the row that the cursor is on.
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let laidout_text_height = laidout_text.size_in_lpxs.height as f64;
        let position = self.cursor_to_position(self.cursor()).unwrap();
        let laidout_row = &laidout_text.rows[position.row_index];
        let y_min = (laidout_row.origin_in_lpxs.y - laidout_row.ascender_in_lpxs) as f64;
        let y_max = (laidout_row.origin_in_lpxs.y - laidout_row.descender_in_lpxs) as f64;

        // If the min y of the row is less than the scroll position, scroll up so that the top of
        // the row appears at the top.
        if y_min < self.scroll_y {
            self.scroll_y = y_min;
        }

        // If the max y of the row is greater than the scroll position, scroll down so that the
        // bottom of the row appears at the bottom.
        if y_max > self.scroll_y + height {
            self.scroll_y = y_max - height;
        }

        // Clamp the scroll position so that we cannot scroll past the start or end of the text.
        let max_scroll_y = laidout_text_height.max(height) - height;
        self.scroll_y = self.scroll_y.max(0.0).min(max_scroll_y);

        // Shift the align range of the turtle with the scroll position, but do not include the
        // begin entry, since that would also scroll the background.
        let align_range: TurtleAlignRange = cx.get_turtle_align_range();
        cx.shift_align_range(
            &TurtleAlignRange {
                start: align_range.start + 1,
                end: align_range.end,
            },
            dvec2(0.0, -self.scroll_y),
        );
    }

    /// Moves the cursor one column to the left.
    ///
    /// Returns `true` if the cursor/selection actually changed.
    pub fn move_cursor_left(&mut self, cx: &mut Cx, keep_selection: bool) -> bool {
        let initial = self.selection;
        self.set_cursor(
            cx,
            Cursor {
                index: prev_grapheme_boundary(&self.text, self.selection.cursor.index),
                prefer_next_row: true,
            },
            keep_selection,
        );
        !initial.index_eq(self.selection)
    }

    /// Moves the cursor one column to the right.
    ///
    /// Returns `true` if the cursor/selection actually changed.
    pub fn move_cursor_right(&mut self, cx: &mut Cx, keep_selection: bool) -> bool {
        let initial = self.selection;
        self.set_cursor(
            cx,
            Cursor {
                index: next_grapheme_boundary(&self.text, self.selection.cursor.index),
                prefer_next_row: false,
            },
            keep_selection,
        );
        !initial.index_eq(self.selection)
    }

    /// Moves the cursor one line (row) up.
    ///
    /// * Returns Ok(`true`) if the cursor/selection actually changed.
    /// * Returns Ok(`false`) if the cursor/selection movement was properly handled but did not change,
    ///   e.g., if the cursor was already at the top-most row.
    /// * Returns `Err` if the cursor/selection failed to be calculated due to a prior layout invalidation.
    pub fn move_cursor_up(&mut self, cx: &mut Cx, keep_selection: bool) -> Result<bool, ()> {
        let initial = self.selection;
        let position = self.cursor_to_position(self.selection.cursor)?;
        self.set_cursor(
            cx,
            self.position_to_cursor(CursorPosition {
                row_index: if position.row_index == 0 {
                    0
                } else {
                    position.row_index - 1
                },
                x_in_lpxs: position.x_in_lpxs,
            })?,
            keep_selection,
        );
        Ok(!initial.index_eq(self.selection))
    }

    /// Moves the cursor one line (row) down.
    ///
    /// * Returns Ok(`true`) if the cursor/selection actually changed.
    /// * Returns Ok(`false`) if the cursor/selection movement was properly handled but did not change,
    ///   e.g., if the cursor was already at the bottom-most row.
    /// * Returns Err(`()`) if the cursor/selection failed to be calculated due to a prior layout invalidation.
    pub fn move_cursor_down(&mut self, cx: &mut Cx, keep_selection: bool) -> Result<bool, ()> {
        let initial = self.selection;
        let laidout_text = self.laidout_text.as_ref().unwrap();
        let position = self.cursor_to_position(self.selection.cursor)?;
        self.set_cursor(
            cx,
            self.position_to_cursor(CursorPosition {
                row_index: if position.row_index == laidout_text.rows.len() - 1 {
                    laidout_text.rows.len() - 1
                } else {
                    position.row_index + 1
                },
                x_in_lpxs: position.x_in_lpxs,
            })?,
            keep_selection,
        );
        Ok(!initial.index_eq(self.selection))
    }

    pub fn select_all(&mut self, cx: &mut Cx) {
        self.set_selection(
            cx,
            Selection {
                anchor: Cursor {
                    index: 0,
                    prefer_next_row: false,
                },
                cursor: Cursor {
                    index: self.text.len(),
                    prefer_next_row: false,
                },
            },
        );
    }

    pub fn select_word(&mut self, cx: &mut Cx) {
        if self.selection.cursor.index < self.selection.anchor.index {
            self.set_cursor(
                cx,
                Cursor {
                    index: self.ceil_word_boundary(self.selection.cursor.index),
                    prefer_next_row: true,
                },
                true,
            );
        } else if self.selection.cursor.index > self.selection.anchor.index {
            self.set_cursor(
                cx,
                Cursor {
                    index: self.floor_word_boundary(self.selection.cursor.index),
                    prefer_next_row: false,
                },
                true,
            );
        } else {
            self.set_selection(
                cx,
                Selection {
                    anchor: Cursor {
                        index: self.ceil_word_boundary(self.selection.cursor.index),
                        prefer_next_row: true,
                    },
                    cursor: Cursor {
                        index: self.floor_word_boundary(self.selection.cursor.index),
                        prefer_next_row: false,
                    },
                },
            );
        }
    }

    pub fn force_new_edit_group(&mut self) {
        self.history.force_new_edit_group();
    }

    fn handle_focus_lost(&mut self, cx: &mut Cx, scope_path: &HeapLiveIdPath, uid: WidgetUid) {
        self.animator_play(cx, ids!(focus.off));
        self.animator_play(cx, ids!(blink.on));
        cx.stop_timer(self.blink_timer);
        cx.hide_text_ime();
        self.composition_start = 0;
        self.composition_end = 0;
        match cx.os_type() {
            OsType::Android(_) | OsType::Ios(_) => {
                cx.hide_clipboard_actions();
            }
            _ => {}
        }
        cx.widget_action(uid, scope_path, TextInputAction::KeyFocusLost);
    }

    fn ceil_word_boundary(&self, index: usize) -> usize {
        let mut prev_word_boundary_index = 0;
        for (word_boundary_index, _) in self.text.split_word_bound_indices() {
            if word_boundary_index > index {
                return prev_word_boundary_index;
            }
            prev_word_boundary_index = word_boundary_index;
        }
        prev_word_boundary_index
    }

    fn floor_word_boundary(&self, index: usize) -> usize {
        let mut prev_word_boundary_index = self.text.len();
        for (word_boundary_index, _) in self.text.split_word_bound_indices().rev() {
            if word_boundary_index < index {
                return prev_word_boundary_index;
            }
            prev_word_boundary_index = word_boundary_index;
        }
        prev_word_boundary_index
    }

    fn filter_input(&self, input: &str, is_set_text: bool) -> String {
        // strip out escape sequences and tabs sometimes sent from the IME
        if input.len() == 1 && input.chars().next().unwrap() <= '\u{1d}' {
            return String::new();
        }

        // Filter based on input_mode
        match self.input_mode {
            InputMode::Ascii => {
                // ASCII only: characters with code point < 128
                input.chars().filter(|c| c.is_ascii()).collect()
            }
            InputMode::Numeric => {
                // Digits only
                input.chars().filter(|c| c.is_ascii_digit()).collect()
            }
            InputMode::Decimal => {
                // Digits, decimal point, and sign
                let mut contains_dot = if is_set_text {
                    false
                } else {
                    let before_selection = self.text[..self.selection.start().index].to_string();
                    let after_selection = self.text[self.selection.end().index..].to_string();
                    before_selection.contains('.') || after_selection.contains('.')
                };
                input
                    .chars()
                    .filter(|c| match c {
                        '.' if !contains_dot => {
                            contains_dot = true;
                            true
                        }
                        '-' | '+' => true,
                        c => c.is_ascii_digit(),
                    })
                    .collect()
            }
            InputMode::Tel => {
                // Digits and common phone characters
                input
                    .chars()
                    .filter(|c| {
                        c.is_ascii_digit() || matches!(c, '+' | '-' | ' ' | '(' | ')' | '*' | '#')
                    })
                    .collect()
            }
            // Text, Url, Email, Search - allow everything
            InputMode::Text | InputMode::Url | InputMode::Email | InputMode::Search => {
                input.to_string()
            }
        }
    }

    fn create_or_extend_edit_group(&mut self, edit_kind: EditKind) {
        self.history
            .create_or_extend_edit_group(edit_kind, self.selection);
    }

    fn apply_edit(&mut self, cx: &mut Cx, edit: Edit) {
        self.selection.cursor.index = edit.start + edit.replace_with.len();
        self.selection.anchor.index = self.selection.cursor.index;
        self.history.apply_edit(edit, &mut self.text);
        self.laidout_text = None;
        self.check_text_is_empty(cx);
    }

    fn undo(&mut self, cx: &mut Cx) -> bool {
        if let Some(new_selection) = self.history.undo(self.selection, &mut self.text) {
            self.laidout_text = None;
            self.selection = new_selection;
            self.check_text_is_empty(cx);
            true
        } else {
            false
        }
    }

    fn redo(&mut self, cx: &mut Cx) -> bool {
        if let Some(new_selection) = self.history.redo(self.selection, &mut self.text) {
            self.laidout_text = None;
            self.selection = new_selection;
            self.check_text_is_empty(cx);
            true
        } else {
            false
        }
    }

    fn check_text_is_empty(&mut self, cx: &mut Cx) {
        if self.text.is_empty() {
            self.animator_play(cx, ids!(empty.on));
        } else {
            self.animator_play(cx, ids!(empty.off));
        }
    }
}

impl Widget for TextInput {
    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, text: &str) {
        self.text = self.filter_input(text, true);
        self.set_selection(
            cx,
            Selection {
                anchor: Cursor {
                    index: self.selection.anchor.index.min(self.text.len()),
                    prefer_next_row: self.selection.anchor.prefer_next_row,
                },
                cursor: Cursor {
                    index: self.selection.cursor.index.min(self.text.len()),
                    prefer_next_row: self.selection.cursor.prefer_next_row,
                },
            },
        );
        self.history.clear();
        self.laidout_text = None;
        self.draw_bg.redraw(cx);
        self.check_text_is_empty(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_selection.append_to_draw_call(cx);
        self.draw_composition_underline.append_to_draw_call(cx);
        self.layout_text(cx);
        let text_rect = self.draw_text(cx);
        let cursor_rect = self.draw_cursor(cx, text_rect);
        self.draw_selection(cx, text_rect);
        self.draw_composition_underline(cx, text_rect);
        self.scroll_to_cursor(cx);
        self.draw_bg.end(cx);
        if cx.has_key_focus(self.draw_bg.area()) {
            // ECHO PREVENTION: Skip if we received IME input this frame.
            // The frame counter (ime_update_frame) is set when we process TextInput events.
            // If it matches current redraw_id, the IME just sent us state - don't echo it back.
            if self.ime_update_frame != cx.redraw_id() {
                self.update_ime_context(cx);
            }

            let cursor_bottom_pos = cursor_rect.pos + cursor_rect.size;
            cx.show_text_ime_with_config(
                self.draw_bg.area(),
                dvec2(cursor_bottom_pos.x, cursor_bottom_pos.y - self.scroll_y),
                self.get_ime_config(),
            );
        }
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Margin::default());
        DrawStep::done()
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(
            cx,
            disabled,
            Animate::Yes,
            ids!(disabled.on),
            ids!(disabled.off),
        );
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        if self.blink_timer.is_event(event).is_some() {
            if self.animator_in_state(cx, ids!(blink.off)) {
                self.animator_play(cx, ids!(blink.on));
            } else {
                self.animator_play(cx, ids!(blink.off));
            }
            self.blink_timer = cx.start_timeout(self.blink_speed)
        }

        let uid = self.widget_uid();

        // Self-detect focus loss from taps outside our area
        if cx.has_key_focus(self.draw_bg.area()) {
            let rect = self.draw_bg.area().rect(cx);
            let should_lose_focus = match event {
                // Handle desktop mouse clicks
                Event::MouseUp(mu) => !rect.contains(mu.abs),
                // Handle mobile touch events
                Event::TouchUpdate(tu) => {
                    // Check if any touch ended outside our area
                    tu.touches.iter().any(|touch| {
                        matches!(touch.state, TouchState::Stop) && !rect.contains(touch.abs)
                    })
                }
                _ => false,
            };

            if should_lose_focus {
                // Update focus state in cx
                cx.set_key_focus(Area::Empty);
                // Handle focus loss locally
                self.handle_focus_lost(cx, &scope.path, uid);
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::KeyFocus(_) => {
                use crate::makepad_platform::event::keyboard::CharOffset;

                self.animator_play(cx, ids!(focus.on));
                self.reset_blink_timer(cx);

                // Immediately sync text state to platform IME when gaining focus
                // This ensures the platform gets correct text BEFORE keyboard is shown
                // Works for both Android (UTF-16 conversion in platform layer) and iOS
                let sel_start_chars = self.text[..self.selection.start().index].chars().count();
                let sel_end_chars = self.text[..self.selection.end().index].chars().count();
                cx.sync_ime_state(
                    self.text.clone(),
                    CharOffset(sel_start_chars)..CharOffset(sel_end_chars),
                    None,
                );

                // Update cache to match what we just sent
                self.last_sent_ime_text = self.text.clone();
                self.last_sent_ime_sel_start = self.selection.start().index;
                self.last_sent_ime_sel_end = self.selection.end().index;
                cx.widget_action(uid, &scope.path, TextInputAction::KeyFocus);
            }
            Hit::KeyFocusLost(_) => {
                self.handle_focus_lost(cx, &scope.path, uid);
            }
            Hit::KeyDown(
                kev @ KeyEvent {
                    key_code: KeyCode::ArrowLeft,
                    modifiers:
                        KeyModifiers {
                            shift: keep_selection,
                            logo: false,
                            alt: false,
                            control: false,
                        },
                    ..
                },
            ) => {
                self.reset_blink_timer(cx);
                let did_move = self.move_cursor_left(cx, keep_selection);
                if !did_move {
                    cx.widget_action(uid, &scope.path, TextInputAction::KeyDownUnhandled(kev));
                }
            }
            Hit::KeyDown(
                kev @ KeyEvent {
                    key_code: KeyCode::ArrowRight,
                    modifiers:
                        KeyModifiers {
                            shift: keep_selection,
                            logo: false,
                            alt: false,
                            control: false,
                        },
                    ..
                },
            ) => {
                self.reset_blink_timer(cx);
                let did_move = self.move_cursor_right(cx, keep_selection);
                if !did_move {
                    cx.widget_action(uid, &scope.path, TextInputAction::KeyDownUnhandled(kev));
                }
            }
            Hit::KeyDown(
                kev @ KeyEvent {
                    key_code: KeyCode::ArrowUp,
                    modifiers:
                        KeyModifiers {
                            shift: keep_selection,
                            logo: false,
                            alt: false,
                            control: false,
                        },
                    ..
                },
            ) => {
                self.reset_blink_timer(cx);
                match self.move_cursor_up(cx, keep_selection) {
                    Ok(true) => {}
                    Ok(false) => {
                        cx.widget_action(uid, &scope.path, TextInputAction::KeyDownUnhandled(kev))
                    }
                    Err(_) => warning!(
                        "can't move cursor up because layout was invalidated by earlier event"
                    ),
                }
            }
            Hit::KeyDown(
                kev @ KeyEvent {
                    key_code: KeyCode::ArrowDown,
                    modifiers:
                        KeyModifiers {
                            shift: keep_selection,
                            logo: false,
                            alt: false,
                            control: false,
                        },
                    ..
                },
            ) => {
                self.reset_blink_timer(cx);
                match self.move_cursor_down(cx, keep_selection) {
                    Ok(true) => {}
                    Ok(false) => {
                        cx.widget_action(uid, &scope.path, TextInputAction::KeyDownUnhandled(kev))
                    }
                    Err(_) => warning!(
                        "can't move cursor down because layout was invalidated by earlier event"
                    ),
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyA,
                modifiers,
                ..
            }) if modifiers.is_primary() => {
                self.select_all(cx);
                // Show clipboard actions after select all
                let has_selection = !self.selected_text().is_empty();
                let selection_rect = self.get_selection_rect(cx);
                cx.show_clipboard_actions(has_selection, selection_rect, cx.keyboard_shift);
            }
            Hit::FingerDown(FingerDownEvent {
                abs,
                tap_count,
                device,
                ..
            }) if device.is_primary_hit() => {
                self.reset_blink_timer(cx);
                self.set_key_focus(cx);
                let rel = abs - self.text_area.rect(cx).pos;
                let Ok(cursor) =
                    self.point_in_lpxs_to_cursor(Point::new(rel.x as f32, rel.y as f32))
                else {
                    warning!("can't move cursor because layout was invalidated by earlier event");
                    return;
                };

                let selection = self.selection();
                let has_selection = selection.cursor != selection.anchor;
                let touching_selection = if has_selection {
                    let sel_start = selection.start().index;
                    let sel_end = selection.end().index;
                    cursor.index >= sel_start && cursor.index <= sel_end
                } else {
                    false
                };

                if tap_count > 1 || !touching_selection {
                    self.set_cursor(cx, cursor, false);
                    self.preserved_selection_cursor = None;
                } else {
                    self.preserved_selection_cursor = Some(cursor);
                }

                match tap_count {
                    2 => {
                        self.select_word(cx);
                        if device.is_touch() {
                            let has_selection = !self.selected_text().is_empty();
                            let selection_rect = self.get_selection_rect(cx);
                            cx.show_clipboard_actions(
                                has_selection,
                                selection_rect,
                                cx.keyboard_shift,
                            );
                        }
                    }
                    3 => {
                        self.select_all(cx);
                        if device.is_touch() {
                            let has_selection = !self.selected_text().is_empty();
                            let selection_rect = self.get_selection_rect(cx);
                            cx.show_clipboard_actions(
                                has_selection,
                                selection_rect,
                                cx.keyboard_shift,
                            );
                        }
                    }
                    _ => {
                        // Single tap - hide clipboard actions popup if shown
                        if device.is_touch() {
                            cx.hide_clipboard_actions();
                        }
                    }
                }

                self.animator_play(cx, ids!(hover.down));
            }
            Hit::FingerUp(fe) => {
                self.ignore_next_move = false;

                if fe.was_tap() {
                    if let Some(cursor) = self.preserved_selection_cursor.take() {
                        self.set_cursor(cx, cursor, false);
                    }
                } else {
                    self.preserved_selection_cursor = None;
                }

                if fe.is_over && fe.was_tap() {
                    if fe.has_hovers() {
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            Hit::FingerLongPress(lp) => {
                self.preserved_selection_cursor = None;

                // Select word at long press position
                let rel = lp.abs - self.text_area.rect(cx).pos;
                if let Ok(cursor) =
                    self.point_in_lpxs_to_cursor(Point::new(rel.x as f32, rel.y as f32))
                {
                    // Check if cursor is over actual text
                    if cursor.index < self.text.len() {
                        self.set_cursor(cx, cursor, false);
                        self.select_word(cx);
                    } else {
                        // Long press on empty space just position the cursor
                        self.set_cursor(cx, cursor, false);
                    }
                }

                // Show clipboard actions menu with updated selection
                if lp.device.is_touch() {
                    let has_selection = !self.selected_text().is_empty();
                    let selection_rect = self.get_selection_rect(cx);
                    cx.show_clipboard_actions(has_selection, selection_rect, cx.keyboard_shift);
                }

                // Skip next move to prevent selection change when finger lifts
                self.ignore_next_move = true;
            }
            Hit::FingerMove(FingerMoveEvent {
                abs,
                tap_count,
                device,
                ..
            }) if device.is_primary_hit() => {
                // Skip first move after long press to prevent selection changes
                if self.ignore_next_move {
                    self.ignore_next_move = false;
                    return;
                }

                // Clear preserved cursor - user is dragging to select
                self.preserved_selection_cursor = None;
                self.reset_blink_timer(cx);
                self.set_key_focus(cx);
                let rel = abs - self.text_area.rect(cx).pos;
                let Ok(cursor) =
                    self.point_in_lpxs_to_cursor(Point::new(rel.x as f32, rel.y as f32))
                else {
                    warning!("can't move cursor because layout was invalidated by earlier event");
                    return;
                };
                self.set_cursor(cx, cursor, true);
                match tap_count {
                    2 => self.select_word(cx),
                    3 => self.select_all(cx),
                    _ => {}
                }
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: mods @ KeyModifiers { shift: false, .. },
                ..
            }) => {
                // For multiline text input, plain Return inserts a newline
                // For single-line, Return emits the Returned action (submit)
                if self.is_multiline && !self.is_read_only {
                    self.reset_blink_timer(cx);
                    self.create_or_extend_edit_group(EditKind::Other);
                    self.apply_edit(
                        cx,
                        Edit {
                            start: self.selection.start().index,
                            end: self.selection.end().index,
                            replace_with: "\n".to_string(),
                        },
                    );
                    self.draw_bg.redraw(cx);
                    cx.widget_action(
                        uid,
                        &scope.path,
                        TextInputAction::Changed(self.text.clone()),
                    );
                } else {
                    cx.hide_text_ime();
                    cx.widget_action(
                        uid,
                        &scope.path,
                        TextInputAction::Returned(self.text.clone(), mods),
                    );
                }
            }

            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Escape,
                ..
            }) => {
                cx.widget_action(uid, &scope.path, TextInputAction::Escaped);
            }
            // Shift+Return always inserts newline (even in single-line mode for backwards compat)
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::ReturnKey,
                modifiers: KeyModifiers { shift: true, .. },
                ..
            }) if !self.is_read_only => {
                self.reset_blink_timer(cx);
                self.create_or_extend_edit_group(EditKind::Other);
                self.apply_edit(
                    cx,
                    Edit {
                        start: self.selection.start().index,
                        end: self.selection.end().index,
                        replace_with: "\n".to_string(),
                    },
                );
                self.draw_bg.redraw(cx);
                cx.widget_action(
                    uid,
                    &scope.path,
                    TextInputAction::Changed(self.text.clone()),
                );
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Backspace,
                ..
            }) if !self.is_read_only => {
                self.reset_blink_timer(cx);
                let mut start = self.selection.start().index;
                let end = self.selection.end().index;
                if start == end {
                    start = prev_grapheme_boundary(&self.text, start);
                }
                self.create_or_extend_edit_group(EditKind::Backspace);
                self.apply_edit(
                    cx,
                    Edit {
                        start,
                        end,
                        replace_with: String::new(),
                    },
                );
                self.draw_bg.redraw(cx);
                cx.widget_action(
                    uid,
                    &scope.path,
                    TextInputAction::Changed(self.text.clone()),
                );
                cx.hide_clipboard_actions();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::Delete,
                ..
            }) if !self.is_read_only => {
                self.reset_blink_timer(cx);
                let start = self.selection.start().index;
                let mut end = self.selection.end().index;
                if start == end {
                    end = next_grapheme_boundary(&self.text, end);
                }
                self.create_or_extend_edit_group(EditKind::Delete);
                self.apply_edit(
                    cx,
                    Edit {
                        start,
                        end,
                        replace_with: String::new(),
                    },
                );
                self.draw_bg.redraw(cx);
                cx.widget_action(
                    uid,
                    &scope.path,
                    TextInputAction::Changed(self.text.clone()),
                );
                cx.hide_clipboard_actions();
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: modifiers @ KeyModifiers { shift: false, .. },
                ..
            }) if modifiers.is_primary() && !self.is_read_only => {
                if !self.undo(cx) {
                    return;
                }
                self.draw_bg.redraw(cx);
                cx.widget_action(
                    uid,
                    &scope.path,
                    TextInputAction::Changed(self.text.clone()),
                );
            }
            Hit::KeyDown(KeyEvent {
                key_code: KeyCode::KeyZ,
                modifiers: modifiers @ KeyModifiers { shift: true, .. },
                ..
            }) if modifiers.is_primary() && !self.is_read_only => {
                if !self.redo(cx) {
                    return;
                }
                self.draw_bg.redraw(cx);
                cx.widget_action(
                    uid,
                    &scope.path,
                    TextInputAction::Changed(self.text.clone()),
                );
            }
            Hit::TextInput(event) if !self.is_read_only => {
                // Text changes invalidate any preserved cursor from a pending tap gesture
                self.preserved_selection_cursor = None;
                // Unified text input handler for all platforms
                // Handle Android full state sync (authoritative from Java InputConnection)
                if let Some(full_state) = &event.full_state_sync {
                    let text_changed = self.text != full_state.text;
                    if text_changed {
                        self.history
                            .create_or_extend_edit_group(EditKind::Other, self.selection);
                        self.text = full_state.text.clone();
                        self.laidout_text = None;
                    }

                    // Update selection from platform
                    let sel_start_byte = full_state.selection.start.to_byte_index(&self.text);
                    let sel_end_byte = full_state.selection.end.to_byte_index(&self.text);
                    self.selection = Selection {
                        anchor: Cursor {
                            index: sel_start_byte,
                            prefer_next_row: false,
                        },
                        cursor: Cursor {
                            index: sel_end_byte,
                            prefer_next_row: false,
                        },
                    };

                    // Update composition from platform
                    if let Some(composition_range) = &full_state.composition {
                        self.composition_start = composition_range.start.to_byte_index(&self.text);
                        self.composition_end = composition_range.end.to_byte_index(&self.text);
                    } else {
                        self.composition_start = 0;
                        self.composition_end = 0;
                    }

                    // Track sent state to prevent sync loops (using byte indices for efficiency)
                    self.last_sent_ime_text = self.text.clone();
                    self.last_sent_ime_sel_start = sel_start_byte;
                    self.last_sent_ime_sel_end = sel_end_byte;
                    self.ime_update_frame = cx.redraw_id();

                    self.draw_bg.redraw(cx);
                    cx.widget_action(
                        uid,
                        &scope.path,
                        TextInputAction::Changed(self.text.clone()),
                    );
                    if text_changed {
                        cx.hide_clipboard_actions();
                    }
                    return;
                }

                // Handle iOS range replacement (autocorrect/paste)
                // iOS uses a different model than Android: instead of sending full state,
                // iOS's UITextInput sends `replaceRange:withText:` for autocorrect and paste.
                // This specifies an exact range to replace, which may NOT match the current
                // selection (e.g., autocorrecting "teh" to "the" while cursor is elsewhere).
                // Android handles equivalent operations via full_state_sync above.
                if let Some((start, end)) = event.replace_range {
                    let filtered_text = self.filter_input(&event.input, false);
                    // Input filtering: if all characters were filtered out but input wasn't
                    // empty, the input was invalid for this field (e.g., letters in numeric-only).
                    // We re-sync to reject it.
                    if filtered_text.is_empty() && !event.input.is_empty() {
                        self.update_ime_context(cx);
                        return;
                    }

                    // Convert character offsets to byte indices
                    let byte_start = start.to_byte_index(&self.text);
                    let byte_end = end.to_byte_index(&self.text);

                    // Adjust composition_start if edit was before active composition
                    if self.has_composition() && byte_start < self.composition_start {
                        let edit_delta =
                            filtered_text.len() as isize - (byte_end - byte_start) as isize;
                        self.composition_start =
                            (self.composition_start as isize + edit_delta).max(0) as usize;
                    }
                    self.composition_end = self.composition_start;
                    self.create_or_extend_edit_group(EditKind::Other);
                    self.apply_edit(
                        cx,
                        Edit {
                            start: byte_start,
                            end: byte_end,
                            replace_with: filtered_text,
                        },
                    );
                    // Do not sync back to platform, since the platform IME already knows the composition text.
                    // Otherwise syncing back might clear the iOS buffer and cause it to lose the pending trigger character
                    // (space, period, etc.) that iOS was about to insert after autocorrect.
                    self.ime_update_frame = cx.redraw_id();

                    self.animator_play(cx, ids!(empty.off));
                    self.draw_bg.redraw(cx);
                    cx.widget_action(
                        uid,
                        &scope.path,
                        TextInputAction::Changed(self.text.clone()),
                    );
                    cx.hide_clipboard_actions();
                    return;
                }

                // Handle regular text input and composition (all platforms)
                let input = self.filter_input(&event.input, false);
                if input.is_empty() {
                    // Composition cancelled, remove preview text
                    if event.replace_last && self.has_composition() {
                        self.create_or_extend_edit_group(EditKind::Other);
                        self.apply_edit(
                            cx,
                            Edit {
                                start: self.composition_start.min(self.text.len()),
                                end: self.composition_end.min(self.text.len()),
                                replace_with: String::new(),
                            },
                        );
                        self.draw_bg.redraw(cx);
                        cx.widget_action(
                            uid,
                            &scope.path,
                            TextInputAction::Changed(self.text.clone()),
                        );
                    }
                    self.composition_end = self.composition_start;
                    return;
                }

                if event.replace_last {
                    // IME composition preview
                    if self.has_composition() {
                        // Replace previous composition text
                        let start = self.composition_start.min(self.text.len());
                        let end = self.composition_end.min(self.text.len());
                        self.create_or_extend_edit_group(EditKind::Other);
                        self.apply_edit(
                            cx,
                            Edit {
                                start,
                                end,
                                replace_with: input.clone(),
                            },
                        );
                        self.composition_end = self.composition_start + input.len();
                    } else {
                        // First composition character, record start position
                        self.composition_start = self.selection.start().index;
                        self.composition_end = self.composition_start + input.len();
                        self.create_or_extend_edit_group(EditKind::Other);
                        self.apply_edit(
                            cx,
                            Edit {
                                start: self.selection.start().index,
                                end: self.selection.end().index,
                                replace_with: input,
                            },
                        );
                    }
                    self.ime_update_frame = cx.redraw_id();
                } else {
                    // Final commit or regular text input
                    if self.has_composition() {
                        // Replace composition with final committed text
                        let start = self.composition_start.min(self.text.len());
                        let end = self.composition_end.min(self.text.len());
                        self.create_or_extend_edit_group(EditKind::Other);
                        self.apply_edit(
                            cx,
                            Edit {
                                start,
                                end,
                                replace_with: input,
                            },
                        );
                        self.composition_end = self.composition_start;
                    } else {
                        // Normal text input (no active composition)
                        self.create_or_extend_edit_group(if event.was_paste {
                            EditKind::Other
                        } else {
                            EditKind::Insert
                        });
                        self.apply_edit(
                            cx,
                            Edit {
                                start: self.selection.start().index,
                                end: self.selection.end().index,
                                replace_with: input,
                            },
                        );
                    }
                }
                self.animator_play(cx, ids!(empty.off));
                self.draw_bg.redraw(cx);
                cx.widget_action(
                    uid,
                    &scope.path,
                    TextInputAction::Changed(self.text.clone()),
                );
                cx.hide_clipboard_actions();
            }
            Hit::ImeAction(event) => {
                // Mobile keyboard action button (Done, Go, Search, etc.)
                use crate::makepad_platform::event::ImeAction;
                let mods = KeyModifiers::default();
                match event.action {
                    // Actions that should hide keyboard and release focus
                    ImeAction::Done | ImeAction::Go | ImeAction::Search | ImeAction::Send => {
                        cx.hide_text_ime();
                        cx.revert_key_focus();
                        cx.widget_action(
                            uid,
                            &scope.path,
                            TextInputAction::Returned(self.text.clone(), mods),
                        );
                    }
                    ImeAction::Next | ImeAction::Previous => {
                        // These actions indicate form field navigation (e.g., "Next" button
                        // on a keyboard that moves to the next text field). We emit Returned
                        // so the parent form can handle field navigation, but unlike Done/Go,
                        // we don't hide the keyboard or release focus since the keyboard
                        // should remain visible for the next field.
                        //
                        // TODO: Implement proper field navigation, perhaps emitting another action here
                        // that is used somewhere to swap focus to the next field.
                        cx.widget_action(
                            uid,
                            &scope.path,
                            TextInputAction::Returned(self.text.clone(), mods),
                        );
                    }
                    ImeAction::Unspecified | ImeAction::None => {}
                }
            }
            Hit::TextCopy(event) => {
                *event.response.borrow_mut() = Some(self.selected_text().to_string());
            }
            Hit::TextCut(event) => {
                *event.response.borrow_mut() = Some(self.selected_text().to_string());
                if !self.selected_text().is_empty() {
                    self.history
                        .create_or_extend_edit_group(EditKind::Other, self.selection);
                    self.apply_edit(
                        cx,
                        Edit {
                            start: self.selection.start().index,
                            end: self.selection.end().index,
                            replace_with: String::new(),
                        },
                    );
                    self.draw_bg.redraw(cx);
                    cx.widget_action(
                        uid,
                        &scope.path,
                        TextInputAction::Changed(self.text.clone()),
                    );
                }
            }
            Hit::KeyDown(event) => {
                cx.widget_action(uid, &scope.path, TextInputAction::KeyDownUnhandled(event));
            }
            _ => {}
        }
    }
}

impl TextInputRef {
    pub fn is_password(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_password()
        } else {
            false
        }
    }

    pub fn set_is_password(&self, cx: &mut Cx, is_password: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_is_password(cx, is_password);
        }
    }

    pub fn toggle_is_password(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle_is_password(cx);
        }
    }

    pub fn is_read_only(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_read_only()
        } else {
            false
        }
    }

    pub fn set_is_read_only(&self, cx: &mut Cx, is_read_only: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_is_read_only(cx, is_read_only);
        }
    }

    pub fn toggle_is_read_only(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle_is_read_only(cx);
        }
    }

    pub fn empty_text(&self) -> String {
        if let Some(inner) = self.borrow() {
            inner.empty_text().to_string()
        } else {
            String::new()
        }
    }

    pub fn set_empty_text(&self, cx: &mut Cx, empty_text: String) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_empty_text(cx, empty_text);
        }
    }

    pub fn selection(&self) -> Selection {
        if let Some(inner) = self.borrow() {
            inner.selection()
        } else {
            Default::default()
        }
    }

    pub fn set_selection(&self, cx: &mut Cx, selection: Selection) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selection(cx, selection);
        }
    }

    pub fn cursor(&self) -> Cursor {
        if let Some(inner) = self.borrow() {
            inner.cursor()
        } else {
            Default::default()
        }
    }

    pub fn set_cursor(&self, cx: &mut Cx, cursor: Cursor, keep_selection: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_cursor(cx, cursor, keep_selection);
        }
    }

    pub fn selected_text(&self) -> String {
        if let Some(inner) = self.borrow() {
            inner.selected_text().to_string()
        } else {
            String::new()
        }
    }

    pub fn returned(&self, actions: &Actions) -> Option<(String, KeyModifiers)> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()) {
            if let TextInputAction::Returned(text, modifiers) = action {
                return Some((text, modifiers));
            }
        }
        None
    }

    pub fn escaped(&self, actions: &Actions) -> bool {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()) {
            if let TextInputAction::Escaped = action {
                return true;
            }
        }
        false
    }

    pub fn changed(&self, actions: &Actions) -> Option<String> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()) {
            if let TextInputAction::Changed(text) = action {
                return Some(text);
            }
        }
        None
    }

    pub fn key_down_unhandled(&self, actions: &Actions) -> Option<KeyEvent> {
        for action in actions.filter_widget_actions_cast::<TextInputAction>(self.widget_uid()) {
            if let TextInputAction::KeyDownUnhandled(event) = action {
                return Some(event);
            }
        }
        None
    }

    /// Saves the internal state of this text input widget
    /// to a new `TextInputState` object.
    pub fn save_state(&self) -> TextInputState {
        if let Some(inner) = self.borrow() {
            TextInputState {
                text: inner.text.clone(),
                password_text: inner.password_text.clone(),
                selection: inner.selection.clone(),
                history: inner.history.clone(),
            }
        } else {
            TextInputState::default()
        }
    }

    /// Restores the internal state of this text input widget
    /// from the given `TextInputState` object.
    pub fn restore_state(&self, cx: &mut Cx, state: TextInputState) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, &state.text);
            inner.password_text = state.password_text;
            inner.history = state.history;
            inner.set_selection(cx, state.selection);
        }
    }
}

/// The saved (checkpointed) state of a text input widget.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextInputState {
    text: String,
    password_text: String,
    selection: Selection,
    history: History,
}

#[derive(Clone, Debug, DefaultNone)]
pub enum TextInputAction {
    None,
    KeyFocus,
    KeyFocusLost,
    Returned(String, KeyModifiers),
    Escaped,
    Changed(String),
    KeyDownUnhandled(KeyEvent),
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct History {
    current_edit_kind: Option<EditKind>,
    undo_stack: EditStack,
    redo_stack: EditStack,
}

impl History {
    fn force_new_edit_group(&mut self) {
        self.current_edit_kind = None;
    }

    fn create_or_extend_edit_group(&mut self, edit_kind: EditKind, selection: Selection) {
        if !self.current_edit_kind.map_or(false, |current_edit_kind| {
            current_edit_kind.can_merge_with(edit_kind)
        }) {
            self.undo_stack.push_edit_group(selection);
            self.current_edit_kind = Some(edit_kind);
        }
    }

    fn apply_edit(&mut self, edit: Edit, text: &mut String) {
        let inverted_edit = edit.invert(&text);
        edit.apply(text);
        self.undo_stack.push_edit(inverted_edit);
        self.redo_stack.clear();
    }

    fn undo(&mut self, selection: Selection, text: &mut String) -> Option<Selection> {
        if let Some((new_selection, edits)) = self.undo_stack.pop_edit_group() {
            self.redo_stack.push_edit_group(selection);
            for edit in &edits {
                let inverted_edit = edit.invert(text);
                edit.apply(text);
                self.redo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_selection)
        } else {
            None
        }
    }

    fn redo(&mut self, selection: Selection, text: &mut String) -> Option<Selection> {
        if let Some((new_selection, edits)) = self.redo_stack.pop_edit_group() {
            self.undo_stack.push_edit_group(selection);
            for edit in &edits {
                let inverted_edit = edit.invert(text);
                edit.apply(text);
                self.undo_stack.push_edit(inverted_edit);
            }
            self.current_edit_kind = None;
            Some(new_selection)
        } else {
            None
        }
    }

    fn clear(&mut self) {
        self.current_edit_kind = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
enum EditKind {
    Insert,
    Backspace,
    Delete,
    Other,
}

impl EditKind {
    fn can_merge_with(self, other: EditKind) -> bool {
        if self == Self::Other {
            false
        } else {
            self == other
        }
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct EditStack {
    edit_groups: Vec<EditGroup>,
    edits: Vec<Edit>,
}

impl EditStack {
    fn push_edit_group(&mut self, selection: Selection) {
        self.edit_groups.push(EditGroup {
            selection,
            edit_start: self.edits.len(),
        });
    }

    fn push_edit(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    fn pop_edit_group(&mut self) -> Option<(Selection, Vec<Edit>)> {
        match self.edit_groups.pop() {
            Some(edit_group) => Some((
                edit_group.selection,
                self.edits.drain(edit_group.edit_start..).rev().collect(),
            )),
            None => None,
        }
    }

    fn clear(&mut self) {
        self.edit_groups.clear();
        self.edits.clear();
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct EditGroup {
    selection: Selection,
    edit_start: usize,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Edit {
    start: usize,
    end: usize,
    replace_with: String,
}

impl Edit {
    fn apply(&self, text: &mut String) {
        text.replace_range(self.start..self.end, &self.replace_with);
    }

    fn invert(&self, text: &str) -> Self {
        Self {
            start: self.start,
            end: self.start + self.replace_with.len(),
            replace_with: text[self.start..self.end].to_string(),
        }
    }
}

fn prev_grapheme_boundary(text: &str, index: usize) -> usize {
    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.prev_boundary(text, 0).unwrap().unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, index: usize) -> usize {
    let mut cursor = GraphemeCursor::new(index, text.len(), true);
    cursor.next_boundary(text, 0).unwrap().unwrap_or(text.len())
}
