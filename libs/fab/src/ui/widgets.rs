//! Lane D. The Fab control kit: every reusable styled widget the panels
//! compose. Fab density (20 px rows, 4–6 px paddings, 1 px borders,
//! 4 px corners), every control with hover / press / focus / disabled states
//! and a tooltip that carries its shortcut.
//!
//! The rule here is *restyle, never re-implement*: momentary buttons are
//! `Button`, toggles and segmented controls are `RadioButton` (it already
//! owns the `active` animator state), checkboxes are `CheckBox`, tooltips are
//! `Tip` over the shell's single `TipLayer`. The controls that add shell-level
//! behaviour are the drag-numeric field (`dragnum.rs`), overlay popover / pie
//! menu (`popover.rs`, `pie.rs`), and this module's overflowing tab strip.

use makepad_widgets::tip::TipAction;
use makepad_widgets::*;

/// A control's horizontal extent in header-content coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeaderControlSpan {
    pub start: f64,
    pub end: f64,
}

/// Keep a sticky header pan inside the part of the content that can scroll.
pub(crate) fn clamp_header_pan(pan: f64, content_width: f64, visible_width: f64) -> f64 {
    pan.clamp(0.0, (content_width - visible_width).max(0.0))
}

/// Return controls that are not wholly inside the current header viewport.
pub(crate) fn clipped_header_controls(
    spans: &[HeaderControlSpan],
    visible_width: f64,
    pan: f64,
) -> Vec<usize> {
    const EDGE_EPSILON: f64 = 0.1;
    let visible_end = pan + visible_width.max(0.0);
    spans
        .iter()
        .enumerate()
        .filter_map(|(index, span)| {
            (span.start < pan - EDGE_EPSILON || span.end > visible_end + EDGE_EPSILON)
                .then_some(index)
        })
        .collect()
}

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    mod.draw.DrawFabOverflowTab = mod.std.set_type_default() do #(DrawFabOverflowTab::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: vec4(0.0, 0.0, 0.0, 0.0)
        border_color: vec4(0.0, 0.0, 0.0, 0.0)
        radius: 3.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            sdf.fill_keep(self.color)
            sdf.stroke(self.border_color, 1.0)
            return sdf.result
        }
    }

    mod.widgets.FabOverflowTabStripBase = #(FabOverflowTabStrip::register_widget(vm))
    mod.widgets.FabOverflowTabStrip = set_type_default() do mod.widgets.FabOverflowTabStripBase{
        width: Fill
        height: fab.row_height
        clip_x: true
        clip_y: true
        color_bg: vec4(0.0, 0.0, 0.0, 0.0)
        color_tab: fab.color_button
        color_tab_hover: fab.color_button_hover
        color_tab_active: fab.color_accent
        color_border: fab.color_border
        color_text: fab.color_text_dim
        color_text_active: fab.color_text_on_accent
        color_arrow_disabled: fab.color_text_muted
        draw_bg: mod.draw.DrawFabOverflowTab{
            radius: 0.0
        }
        draw_tab: mod.draw.DrawFabOverflowTab{
            radius: fab.radius
        }
        draw_text: mod.draw.DrawText{
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
            color: fab.color_text
        }
        left_icon: FabIconSmall{
            draw_icon +: {
                svg: crate_resource("self://resources/icons/chevron_left.svg")
            }
        }
        right_icon: FabIconSmall{
            draw_icon +: {
                svg: crate_resource("self://resources/icons/chevron_right.svg")
            }
        }
    }

    // =====================================================================
    // Type
    // =====================================================================

    // The stock `Label` carries `padding: theme.mspace_1` (3 px all round),
    // which makes its box ~25 px tall at our 8.5 pt type. In a 20 px Fab
    // row that overflows, `align.y` clamps to 0, and every label lands ~2.5 px
    // below the row's centreline while the buttons beside it (whose own text is
    // `ink_centered`) sit on it. Zero padding, and the ink centres.
    mod.widgets.FabLabel = Label{
        width: Fit
        height: Fit
        padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
        draw_text +: {
            ink_centered: true
            color: fab.color_text
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
    }

    mod.widgets.FabLabelDim = mod.widgets.FabLabel{
        draw_text +: {
            color: fab.color_text_dim
        }
    }

    mod.widgets.FabLabelMuted = mod.widgets.FabLabel{
        draw_text +: {
            color: fab.color_text_muted
        }
    }

    mod.widgets.FabLabelSmall = mod.widgets.FabLabel{
        draw_text +: {
            color: fab.color_text_dim
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
        }
    }

    mod.widgets.FabLabelMono = mod.widgets.FabLabel{
        draw_text +: {
            color: fab.color_text_dim
            text_style: theme.font_code{
                font_size: fab.font_size_small
            }
        }
    }

    mod.widgets.FabHeaderLabel = Label{
        width: Fit
        height: Fit
        padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
        draw_text +: {
            ink_centered: true
            color: fab.color_text_header
            text_style: theme.font_bold{
                font_size: fab.font_size_header
            }
        }
    }

    mod.widgets.FabTitleLabel = Label{
        width: Fit
        height: Fit
        padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
        draw_text +: {
            ink_centered: true
            color: fab.color_text_active
            text_style: theme.font_bold{
                font_size: fab.font_size_title
            }
        }
    }

    // =====================================================================
    // Tooltip wrapper — one `TipLayer{}` lives in the shell overlay.
    // =====================================================================

    mod.widgets.FabTip = Tip{
        width: Fit
        height: Fit
    }

    mod.widgets.FabTipFill = Tip{
        width: Fill
        height: Fit
    }

    // =====================================================================
    // Buttons (momentary) — `Button` owns hover / down / focus / disabled.
    // =====================================================================

    mod.widgets.FabButton = ButtonFlat{
        height: fab.row_height
        width: Fit
        // Text-only: y-align is safe. DrawVector icons must not sit here.
        align: Align{x: 0.5 y: 0.5}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        padding: Inset{left: 9 right: 9 top: 0 bottom: 0}
        label_walk: Walk{ width: Fit height: Fit }
        draw_bg +: {
            color: fab.color_button
            color_hover: fab.color_button_hover
            color_down: fab.color_button_down
            color_focus: fab.color_button
            color_disabled: fab.color_editor_alt
            border_radius: fab.radius
            border_size: 1.0
            border_color: fab.color_border
            border_color_hover: fab.color_border
            border_color_down: fab.color_border
            border_color_focus: fab.color_focus_ring
            border_color_disabled: fab.color_border
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
        draw_text +: {
            ink_centered: true
            color: fab.color_text
            color_hover: fab.color_text_active
            color_down: fab.color_text_active
            color_focus: fab.color_text
            color_disabled: fab.color_text_muted
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.FabButtonAccent = mod.widgets.FabButton{
        draw_bg +: {
            color: fab.color_accent
            color_hover: fab.color_accent_hover
            color_down: fab.color_accent_dim
            color_focus: fab.color_accent
        }
        draw_text +: {
            color: fab.color_text_on_accent
        }
    }

    // Flat momentary text button: no chrome until hovered. Text-only:
    // y-align is safe. (Popup-opening header entries are `FabMenuButton` in
    // `dropdown.rs` — they carry the shared open-state machine.)
    mod.widgets.FabFlatButton = ButtonFlatter{
        height: fab.row_height
        width: Fit
        align: Align{x: 0.5 y: 0.5}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        padding: Inset{left: 7 right: 7 top: 0 bottom: 0}
        label_walk: Walk{ width: Fit height: Fit }
        draw_bg +: {
            color: vec4(0.0, 0.0, 0.0, 0.0)
            color_hover: fab.color_button_hover
            color_down: fab.color_button_down
            color_focus: vec4(0.0, 0.0, 0.0, 0.0)
            border_radius: fab.radius
            border_size: 0.0
        }
        draw_text +: {
            ink_centered: true
            color: fab.color_text
            color_hover: fab.color_text_active
            color_down: fab.color_text_active
            color_focus: fab.color_text
            color_disabled: fab.color_text_muted
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    // Icon-only momentary button (16 px glyph in a 22 px well).
    // No turtle-align: DrawSvg is DrawVector. Centre with padding instead.
    mod.widgets.FabIconButton = ButtonFlatterIcon{
        width: 22
        height: 22
        align: Align{x: 0.0 y: 0.0}
        spacing: 0.0
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        padding: Inset{left: 3 right: 3 top: 3 bottom: 3}
        text: ""
        label_walk: Walk{ width: 0 height: 0 }
        icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
        draw_bg +: {
            color: vec4(0.0, 0.0, 0.0, 0.0)
            color_hover: fab.color_button_hover
            color_down: fab.color_button_down
            color_focus: vec4(0.0, 0.0, 0.0, 0.0)
            border_radius: fab.radius
            border_size: 0.0
        }
        draw_icon +: {
            hover: instance(0.0)
            down: instance(0.0)
            disabled: instance(0.0)
            color: fab.color_text
            color_hover: fab.color_text_active
            color_down: fab.color_text_active
            color_disabled: fab.color_text_muted
            get_color: fn() {
                return self.color
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down)
                    .mix(self.color_disabled, self.disabled)
            }
        }
        // The stock Button animator only drives draw_bg/draw_text; the icon
        // needs the same states or it would sit at one flat tint.
        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_icon: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                        draw_icon: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                        draw_icon: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                        draw_icon: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.FabIconButtonSmall = mod.widgets.FabIconButton{
        width: 16
        height: 16
        padding: Inset{left: 2 right: 2 top: 2 bottom: 2}
        icon_walk: Walk{ width: fab.icon_size_sm height: fab.icon_size_sm }
        draw_icon +: {
            color: fab.color_text_muted
        }
    }

    // =====================================================================
    // Toggles / segmented — `RadioButton` owns the `active` state.
    // =====================================================================

    // Text pill (workspaces, N-panel tabs). Text-only: y-align is safe.
    mod.widgets.FabSegmentTab = RadioButtonTab{
        height: fab.row_height
        width: Fit
        align: Align{x: 0.5 y: 0.5}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        padding: Inset{left: 9 right: 9 top: 0 bottom: 0}
        // RadioButtonTabFlat indents the label 12 px for its (absent) check
        // mark — that is what pushed every pill's ink off-centre.
        label_walk: Walk{ width: Fit height: Fit margin: Inset{left: 0 right: 0 top: 0 bottom: 0} }
        label_align: Align{x: 0.5 y: 0.5}
        icon_walk: Walk{ width: 0 height: 0 }
        draw_bg +: {
            color: fab.color_button
            color_hover: fab.color_button_hover
            color_down: fab.color_button_down
            color_active: fab.color_button_active
            color_focus: fab.color_button
            color_disabled: fab.color_editor_alt
            border_radius: fab.radius
            border_size: 1.0
            border_color: fab.color_border
            border_color_hover: fab.color_border
            border_color_down: fab.color_border
            border_color_active: fab.color_border
            border_color_focus: fab.color_focus_ring
            border_color_disabled: fab.color_border
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
        draw_text +: {
            ink_centered: true
            color: fab.color_text
            color_hover: fab.color_text_active
            color_down: fab.color_text_active
            color_active: fab.color_text_on_accent
            color_focus: fab.color_text
            color_disabled: fab.color_text_muted
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    // Icon toggle (shading modes, x-ray, overlays, lock, tools).
    // 16 px glyph in a 24×20 well: centre with padding, never turtle-align
    // (DrawSvg is DrawVector).
    mod.widgets.FabIconToggle = RadioButtonTab{
        width: 24
        height: fab.row_height
        align: Align{x: 0.0 y: 0.0}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        padding: Inset{left: 4 right: 4 top: 2 bottom: 2}
        text: ""
        label_walk: Walk{ width: 0 height: 0 margin: Inset{left: 0 right: 0 top: 0 bottom: 0} }
        icon_walk: Walk{ width: fab.icon_size height: fab.icon_size }
        draw_bg +: {
            color: vec4(0.0, 0.0, 0.0, 0.0)
            color_hover: fab.color_button_hover
            color_down: fab.color_button_down
            color_active: fab.color_button_active
            color_focus: vec4(0.0, 0.0, 0.0, 0.0)
            color_disabled: vec4(0.0, 0.0, 0.0, 0.0)
            border_radius: fab.radius
            border_size: 1.0
            border_color: vec4(0.0, 0.0, 0.0, 0.0)
            border_color_hover: vec4(0.0, 0.0, 0.0, 0.0)
            border_color_down: vec4(0.0, 0.0, 0.0, 0.0)
            border_color_active: vec4(0.0, 0.0, 0.0, 0.0)
            border_color_focus: fab.color_focus_ring
            border_color_disabled: vec4(0.0, 0.0, 0.0, 0.0)
            border_color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
        }
        draw_icon +: {
            hover: instance(0.0)
            down: instance(0.0)
            active: instance(0.0)
            disabled: instance(0.0)
            color: fab.color_text
            color_hover: fab.color_text_active
            color_down: fab.color_text_active
            color_active: fab.color_text_on_accent
            color_disabled: fab.color_text_muted
            get_color: fn() {
                return self.color
                    .mix(self.color_active, self.active)
                    .mix(self.color_hover, self.hover * (1.0 - self.active))
                    .mix(self.color_disabled, self.disabled)
            }
        }
        animator: Animator{
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {disabled: 0.0}
                        draw_text: {disabled: 0.0}
                        draw_icon: {disabled: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {disabled: 1.0}
                        draw_text: {disabled: 1.0}
                        draw_icon: {disabled: 1.0}
                    }
                }
            }
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 0.0}
                        draw_text: {down: snap(0.0), hover: 0.0}
                        draw_icon: {down: snap(0.0), hover: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(0.0), hover: 1.0}
                        draw_text: {down: snap(0.0), hover: 1.0}
                        draw_icon: {down: snap(0.0), hover: 1.0}
                    }
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_bg: {down: snap(1.0), hover: 1.0}
                        draw_text: {down: snap(1.0), hover: 1.0}
                        draw_icon: {down: snap(1.0), hover: 1.0}
                    }
                }
            }
            active: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                        draw_icon: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                        draw_icon: {active: 1.0}
                    }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: {
                        draw_bg: {focus: 0.0}
                        draw_text: {focus: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {
                        draw_bg: {focus: 1.0}
                        draw_text: {focus: 1.0}
                    }
                }
            }
        }
    }

    // The T toolbar's bigger tool buttons — 20 px glyph in a 28 px well.
    // A header icon that stands for a boolean of its own (Lock Views,
    // X-Ray) rather than one choice among several: it must switch OFF on a
    // second click, which a radio in a group deliberately does not do.
    mod.widgets.FabIconCheck = mod.widgets.FabIconToggle{
        independent: true
    }

    mod.widgets.FabToolToggle = mod.widgets.FabIconToggle{
        width: 28
        height: 28
        padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
        icon_walk: Walk{ width: fab.icon_size_lg height: fab.icon_size_lg }
    }

    // The properties editor's vertical icon strip.
    mod.widgets.FabTabIcon = mod.widgets.FabIconToggle{
        width: 24
        height: 24
        padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
        margin: Inset{bottom: 2 top: 0 left: 0 right: 0}
    }

    // =====================================================================
    // Checkbox — Fab's inset well with a white tick.
    // =====================================================================

    mod.widgets.FabCheckBox = CheckBox{
        height: fab.row_height
        width: Fit
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        label_walk: Walk{ width: Fit height: Fit margin: Inset{left: 18 right: 0 top: 0 bottom: 0} }
        draw_bg +: {
            size: uniform(13.0)
            border_size: uniform(1.0)
            border_radius: uniform(fab.radius)
            color: fab.color_toggle_off
            color_hover: fab.color_input_hover
            color_down: fab.color_input_active
            color_active: fab.color_toggle_on
            color_focus: fab.color_toggle_off
            color_disabled: fab.color_editor_alt
            border_color: fab.color_border
            border_color_hover: fab.color_border_light
            border_color_down: fab.color_border
            border_color_active: fab.color_border
            border_color_focus: fab.color_focus_ring
            border_color_disabled: fab.color_border
            mark_color: vec4(0.0, 0.0, 0.0, 0.0)
            mark_color_hover: vec4(0.0, 0.0, 0.0, 0.0)
            mark_color_down: vec4(0.0, 0.0, 0.0, 0.0)
            mark_color_active: fab.color_toggle_mark
            mark_color_active_hover: fab.color_toggle_mark
            mark_color_focus: vec4(0.0, 0.0, 0.0, 0.0)
            mark_color_disabled: fab.color_text_muted
        }
        draw_text +: {
            ink_centered: true
            color: fab.color_text
            color_hover: fab.color_text_active
            color_active: fab.color_text
            color_disabled: fab.color_text_muted
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
    }

    // =====================================================================
    // Text fields
    // =====================================================================

    mod.widgets.FabInput = TextInput{
        height: fab.row_height
        width: Fill
        padding: Inset{left: 6 right: 6 top: 0 bottom: 0}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
        empty_text: ""
        draw_bg +: {
            color: fab.color_input
            border_radius: fab.radius
        }
        draw_text +: {
            ink_centered: true
            color: fab.color_text
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
    }

    // Search field with the magnifier inside the well.
    mod.widgets.FabSearch = View{
        width: Fill
        height: fab.row_height
        flow: Overlay
        bg := View{
            width: Fill
            height: Fill
            show_bg: true
            draw_bg +: {
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                    sdf.fill_keep(fab.color_input)
                    sdf.stroke(fab.color_border, 1.0)
                    return sdf.result
                }
            }
        }
        row := View{
            width: Fill
            height: Fill
            flow: Right
            align: Align{x: 0.0 y: 0.0}
            padding: Inset{left: 5 right: 4 top: 0 bottom: 0}
            spacing: 4
            glyph_slot := View{
                width: fab.icon_size_sm
                height: Fill
                padding: Inset{top: 4 bottom: 4 left: 0 right: 0}
                glyph := mod.widgets.FabIconMuted{
                    width: fab.icon_size_sm
                    height: fab.icon_size_sm
                    icon_walk: Walk{ width: fab.icon_size_sm height: fab.icon_size_sm }
                    draw_icon +: {
                        svg: crate_resource("self://resources/icons/search.svg")
                    }
                }
            }
            input := TextInput{
                width: Fill
                height: Fill
                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
                empty_text: "Search"
                draw_bg +: {
                    color: vec4(0.0, 0.0, 0.0, 0.0)
                    color_hover: vec4(0.0, 0.0, 0.0, 0.0)
                    color_focus: vec4(0.0, 0.0, 0.0, 0.0)
                    color_down: vec4(0.0, 0.0, 0.0, 0.0)
                    color_empty: vec4(0.0, 0.0, 0.0, 0.0)
                    border_size: 0.0
                    border_radius: 0.0
                }
                draw_text +: {
                    ink_centered: true
                    color: fab.color_text
                    color_empty: fab.color_text_muted
                    color_empty_hover: fab.color_text_dim
                    color_empty_focus: fab.color_text_dim
                    text_style: theme.font_regular{
                        font_size: fab.font_size_ui
                    }
                }
            }
        }
    }

    // =====================================================================
    // Chrome: area headers, panels, rows, rules
    // =====================================================================

    // The 26 px strip at the top of every editor area. Every control owns the
    // same 20 px row; y-align only centres Fit-height text labels. DrawVector
    // children stay in Fill-height, symmetrically padded slots and do not move.
    mod.widgets.FabAreaHeader = View{
        width: Fill
        height: fab.header_height
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 5 right: 6 top: 3 bottom: 3}
        spacing: 4
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                sdf.fill(fab.color_header)
                sdf.rect(0.0, self.rect_size.y - 1.0, self.rect_size.x, 1.0)
                sdf.fill(fab.color_border)
                return sdf.result
            }
        }
    }

    mod.widgets.FabHr = View{
        width: Fill
        height: 1
        show_bg: true
        draw_bg +: {
            color: fab.color_border
        }
    }

    mod.widgets.FabVr = View{
        width: 1
        height: Fill
        show_bg: true
        draw_bg +: {
            color: fab.color_border
        }
    }

    // Panel header: disclosure triangle + title + pin, hover-lit, clickable.
    mod.widgets.FabPanelHeader = View{
        width: Fill
        height: 22
        flow: Right
        // Chevron and title share the row's centre line. Aligned to the top
        // they each sat at their own height and the label rode above the
        // arrow.
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 4 right: 4 top: 0 bottom: 0}
        spacing: 3
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            down: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                sdf.fill(fab.color_panel.mix(fab.color_button_hover, self.hover * 0.5).mix(fab.color_button_down, self.down))
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
            down: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {down: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {down: 1.0} }
                }
            }
        }
        // One chevron, rotated — a second SVG for "closed" would be two files
        // that must agree; `IconRotated` keeps it one glyph and one truth.
        // The row centres its children, so the glyph needs no hand-set pad:
        // a fixed 12x12 slot centred on the same line as the title's ink.
        tri_slot := View{
            width: fab.icon_size_sm
            height: fab.icon_size_sm
            padding: Inset{top: 0 bottom: 0 left: 0 right: 0}
            tri := IconRotated{
                width: fab.icon_size_sm
                height: fab.icon_size_sm
                align: Align{x: 0.5 y: 0.5}
                icon_walk: Walk{ width: fab.icon_size_sm height: fab.icon_size_sm }
                draw_icon +: {
                    color: fab.color_text_dim
                    svg: crate_resource("self://resources/icons/chevron_down.svg")
                    rotation_angle: uniform(0.0)
                }
            }
        }
        title := mod.widgets.FabHeaderLabel{
            height: Fit
            text: "Panel"
        }
        Filler{}
    }

    // A collapsible properties panel: header + animated body.
    //
    // `FoldHeader` flattens its `header` / `body` when children are visited —
    // `self.header.children(visit)`, not `visit(header)` — so a panel's own
    // header would have no path from outside. The header therefore sits in a
    // one-child wrapper: from anywhere above, the header is `<panel>.hdr` and
    // the body's rows are `<panel>.<row>` directly.
    mod.widgets.FabPanel = FoldHeader{
        width: Fill
        height: Fit
        body_walk: Walk{ width: Fill height: Fit }
        header: View{
            width: Fill
            height: Fit
            flow: Down
            hdr := mod.widgets.FabPanelHeader{}
        }
        body: View{
            width: Fill
            height: Fit
            flow: Down
            padding: Inset{left: 0 right: 0 top: 2 bottom: 6}
            spacing: 1
        }
    }

    // Label-left / value-right row.
    mod.widgets.FabPropRow = View{
        width: Fill
        height: fab.row_height
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
        spacing: 6
        // One line each, always: a key or value that does not fit elides —
        // it never wraps into the next row's space.
        name := mod.widgets.FabLabelDim{
            width: fab.prop_label_width
            text: "Name"
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
        }
        value := mod.widgets.FabLabel{
            width: Fill
            text: "—"
            max_lines: 1
            text_overflow: TextOverflow.Ellipsis
        }
    }

    mod.widgets.FabPropRowMono = mod.widgets.FabPropRow{
        value +: {
            draw_text +: {
                color: fab.color_text_dim
                text_style: theme.font_code{
                    font_size: fab.font_size_small
                }
            }
        }
    }

    // A key-cap chip (tooltips, keymap help, palette rows).
    mod.widgets.FabKeyCap = View{
        width: Fit
        height: 16
        flow: Right
        align: Align{x: 0.5 y: 0.5}
        padding: Inset{left: 5 right: 5 top: 0 bottom: 0}
        show_bg: true
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                sdf.fill_keep(fab.color_editor_alt)
                sdf.stroke(fab.color_border_light, 1.0)
                return sdf.result
            }
        }
        cap := mod.widgets.FabLabelSmall{
            text: ""
            draw_text +: {
                color: fab.color_text_dim
                text_style: theme.font_code{
                    font_size: fab.font_size_small
                }
            }
        }
    }

    // Color swatch (materials, sun). Click opens the color popover.
    mod.widgets.FabSwatch = View{
        width: fab.swatch_width
        height: 16
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            swatch: instance(vec4(0.8, 0.8, 0.8, 1.0))
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
                sdf.fill_keep(vec4(self.swatch.xyz, 1.0))
                sdf.stroke(fab.color_border.mix(fab.color_focus_ring, self.hover), 1.0)
                return sdf.result
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0} }
                }
            }
        }
    }

    // Progress bar (load / bake / render).
    mod.widgets.FabProgress = View{
        width: 120
        height: 6
        show_bg: true
        draw_bg +: {
            progress: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 1.5)
                sdf.fill_keep(fab.color_input)
                sdf.stroke(fab.color_border, 1.0)
                sdf.box(1.0, 1.0, max(2.0, (self.rect_size.x - 2.0) * self.progress), self.rect_size.y - 2.0, 1.0)
                sdf.fill(fab.color_accent)
                return sdf.result
            }
        }
    }

    // The scroll body every editor uses.
    mod.widgets.FabScroll = ScrollYView{
        width: Fill
        height: Fill
        flow: Down
        scroll_bars +: {
            show_scroll_x: false
            show_scroll_y: true
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawFabOverflowTab {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live(3.0)]
    radius: f32,
}

/// One entry in [`FabOverflowTabStrip`]. The label may be shortened visually;
/// the tooltip is always kept verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabOverflowTab {
    pub label: String,
    pub tooltip: String,
}

impl FabOverflowTab {
    pub fn new(label: impl Into<String>, tooltip: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tooltip: tooltip.into(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum FabOverflowTabAction {
    Selected(usize),
    #[default]
    None,
}

const TAB_ARROW_WIDTH: f64 = 20.0;
const TAB_GAP: f64 = 2.0;
const TAB_MIN_WIDTH: f64 = 42.0;
const TAB_MAX_WIDTH: f64 = 190.0;
const TAB_TEXT_WIDTH: f64 = 5.4;
const TAB_TEXT_PADDING: f64 = 18.0;
const TAB_SCROLL_STEP: f64 = 72.0;

fn overflow_tab_width(label: &str) -> f64 {
    (label.chars().count() as f64 * TAB_TEXT_WIDTH + TAB_TEXT_PADDING)
        .clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
}

fn reveal_tab(scroll: f64, viewport_width: f64, tab_left: f64, tab_right: f64, max_scroll: f64) -> f64 {
    if tab_left < scroll {
        tab_left.max(0.0)
    } else if tab_right > scroll + viewport_width {
        (tab_right - viewport_width).min(max_scroll)
    } else {
        scroll.clamp(0.0, max_scroll)
    }
}

/// Compact reusable tabs for narrow shell rows. It only adds arrow wells when
/// content actually overflows, accepts vertical or horizontal wheel motion,
/// and reveals a newly active item before painting.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FabOverflowTabStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    color_bg: Vec4f,
    #[live]
    color_tab: Vec4f,
    #[live]
    color_tab_hover: Vec4f,
    #[live]
    color_tab_active: Vec4f,
    #[live]
    color_border: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_text_active: Vec4f,
    #[live]
    color_arrow_disabled: Vec4f,
    #[live]
    draw_bg: DrawFabOverflowTab,
    #[live]
    draw_tab: DrawFabOverflowTab,
    #[live]
    draw_text: DrawText,
    /// Stock `Icon` widgets: both chevrons stay on the shared single-quad SVG path.
    #[live]
    left_icon: WidgetRef,
    #[live]
    right_icon: WidgetRef,
    #[rust]
    area: Area,
    #[rust]
    items: Vec<FabOverflowTab>,
    #[rust]
    active: usize,
    #[rust]
    scroll_x: f64,
    #[rust]
    max_scroll: f64,
    #[rust]
    last_viewport_width: f64,
    #[rust]
    overflow: bool,
    #[rust]
    reveal_active: bool,
    #[rust]
    viewport: Rect,
    #[rust]
    tab_rects: Vec<Rect>,
    #[rust]
    left_arrow: Rect,
    #[rust]
    right_arrow: Rect,
    #[rust]
    hover_tab: Option<usize>,
    #[rust]
    hover_arrow: i8,
    #[rust]
    down_tab: Option<usize>,
    #[rust]
    down_arrow: i8,
}

impl FabOverflowTabStrip {
    pub fn set_tabs(&mut self, cx: &mut Cx, items: Vec<FabOverflowTab>, active: usize) {
        let active = active.min(items.len().saturating_sub(1));
        if self.items == items && self.active == active {
            return;
        }
        self.items = items;
        self.active = active;
        self.reveal_active = true;
        self.area.redraw(cx);
    }

    fn scroll_by(&mut self, cx: &mut Cx, delta: f64) {
        let next = (self.scroll_x + delta).clamp(0.0, self.max_scroll);
        if (next - self.scroll_x).abs() > 0.01 {
            self.scroll_x = next;
            self.reveal_active = false;
            self.area.redraw(cx);
        }
    }

    fn tab_at(&self, abs: DVec2) -> Option<usize> {
        if !self.viewport.contains(abs) {
            return None;
        }
        self.tab_rects
            .iter()
            .enumerate()
            .find(|(_, rect)| rect.intersects(self.viewport) && rect.contains(abs))
            .map(|(index, _)| index)
    }

    fn arrow_at(&self, abs: DVec2) -> i8 {
        if !self.overflow {
            0
        } else if self.left_arrow.contains(abs) {
            -1
        } else if self.right_arrow.contains(abs) {
            1
        } else {
            0
        }
    }

    fn update_hover(&mut self, cx: &mut Cx, abs: DVec2) {
        let tab = self.tab_at(abs);
        let arrow = self.arrow_at(abs);
        if tab != self.hover_tab || arrow != self.hover_arrow {
            if self.hover_tab.is_some() {
                cx.widget_action(self.uid, TipAction::HoverOut);
            }
            if let Some(index) = tab {
                if let (Some(item), Some(rect)) = (self.items.get(index), self.tab_rects.get(index)) {
                    cx.widget_action(
                        self.uid,
                        TipAction::HoverIn(item.tooltip.clone(), *rect),
                    );
                }
            }
            self.hover_tab = tab;
            self.hover_arrow = arrow;
            self.area.redraw(cx);
        }
        cx.set_cursor(if tab.is_some() || arrow != 0 {
            MouseCursor::Hand
        } else {
            MouseCursor::Default
        });
    }

    fn tab_label(label: &str, width: f64) -> String {
        let fits = (((width - TAB_TEXT_PADDING) / TAB_TEXT_WIDTH) as usize).max(3);
        if label.chars().count() > fits {
            label.chars().take(fits.saturating_sub(1)).collect::<String>() + "…"
        } else {
            label.to_string()
        }
    }
}

impl WidgetNode for FabOverflowTabStrip {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for FabOverflowTabStrip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                self.down_tab = self.tab_at(fe.abs);
                self.down_arrow = self.arrow_at(fe.abs);
            }
            Hit::FingerUp(fe) => {
                let tab = self.tab_at(fe.abs);
                let arrow = self.arrow_at(fe.abs);
                if tab.is_some() && tab == self.down_tab {
                    cx.widget_action(self.uid, FabOverflowTabAction::Selected(tab.unwrap()));
                } else if arrow != 0 && arrow == self.down_arrow {
                    self.scroll_by(cx, arrow as f64 * TAB_SCROLL_STEP);
                }
                self.down_tab = None;
                self.down_arrow = 0;
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.update_hover(cx, fe.abs);
            }
            Hit::FingerHoverOut(_) => {
                if self.hover_tab.take().is_some() {
                    cx.widget_action(self.uid, TipAction::HoverOut);
                }
                self.hover_arrow = 0;
                self.area.redraw(cx);
            }
            Hit::FingerScroll(fe) => {
                let delta = if fe.scroll.x.abs() > fe.scroll.y.abs() {
                    fe.scroll.x
                } else {
                    fe.scroll.y
                };
                self.scroll_by(cx, delta);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.color = self.color_bg;
        self.draw_bg.border_color = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_bg.draw_abs(cx, rect);

        let widths: Vec<f64> = self
            .items
            .iter()
            .map(|item| overflow_tab_width(&item.label))
            .collect();
        let content_width = widths.iter().sum::<f64>()
            + TAB_GAP * widths.len().saturating_sub(1) as f64;
        self.overflow = content_width > rect.size.x;
        let arrow_width = if self.overflow { TAB_ARROW_WIDTH } else { 0.0 };
        self.viewport = Rect {
            pos: dvec2(rect.pos.x + arrow_width, rect.pos.y),
            size: dvec2((rect.size.x - arrow_width * 2.0).max(0.0), rect.size.y),
        };
        self.max_scroll = (content_width - self.viewport.size.x).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, self.max_scroll);
        if (self.viewport.size.x - self.last_viewport_width).abs() > 0.1 {
            self.last_viewport_width = self.viewport.size.x;
            self.reveal_active = true;
        }

        let mut tab_left = 0.0;
        for width in widths.iter().take(self.active) {
            tab_left += *width + TAB_GAP;
        }
        if self.reveal_active {
            self.reveal_active = false;
            if let Some(width) = widths.get(self.active) {
                self.scroll_x = reveal_tab(
                    self.scroll_x,
                    self.viewport.size.x,
                    tab_left,
                    tab_left + *width,
                    self.max_scroll,
                );
            }
        }

        self.tab_rects.clear();
        let mut x = self.viewport.pos.x - self.scroll_x;
        cx.push_clip_rect(self.viewport);
        for (index, (item, width)) in self.items.iter().zip(widths.iter()).enumerate() {
            let tab = Rect {
                pos: dvec2(x, rect.pos.y + 1.0),
                size: dvec2(*width, (rect.size.y - 2.0).max(0.0)),
            };
            self.tab_rects.push(tab);
            if tab.intersects(self.viewport) {
                self.draw_tab.color = if index == self.active {
                    self.color_tab_active
                } else if self.hover_tab == Some(index) {
                    self.color_tab_hover
                } else {
                    self.color_tab
                };
                self.draw_tab.border_color = self.color_border;
                self.draw_tab.draw_abs(cx, tab);
                self.draw_text.color = if index == self.active {
                    self.color_text_active
                } else {
                    self.color_text
                };
                let label = Self::tab_label(&item.label, *width);
                self.draw_text
                    .draw_abs(cx, tab.pos + dvec2(9.0, (tab.size.y - 14.0) * 0.5), &label);
            }
            x += *width + TAB_GAP;
        }
        cx.pop_clip_rect();

        if self.overflow {
            self.left_arrow = Rect {
                pos: rect.pos,
                size: dvec2(TAB_ARROW_WIDTH, rect.size.y),
            };
            self.right_arrow = Rect {
                pos: dvec2(rect.pos.x + rect.size.x - TAB_ARROW_WIDTH, rect.pos.y),
                size: dvec2(TAB_ARROW_WIDTH, rect.size.y),
            };
            for (arrow, direction) in [(self.left_arrow, -1), (self.right_arrow, 1)] {
                let enabled = if direction < 0 {
                    self.scroll_x > 0.01
                } else {
                    self.scroll_x < self.max_scroll - 0.01
                };
                self.draw_tab.color = if self.hover_arrow == direction && enabled {
                    self.color_tab_hover
                } else {
                    self.color_tab
                };
                self.draw_tab.border_color = self.color_border;
                self.draw_tab.draw_abs(cx, arrow);
                let icon_rect = Rect {
                    pos: arrow.pos + dvec2(4.0, (arrow.size.y - 12.0) * 0.5),
                    size: dvec2(12.0, 12.0),
                };
                let mut icon = if direction < 0 {
                    self.left_icon.clone()
                } else {
                    self.right_icon.clone()
                };
                let color = if enabled {
                    self.color_text
                } else {
                    self.color_arrow_disabled
                };
                script_apply_eval!(cx, icon, { draw_icon +: { color: #(color) } });
                let _ = icon.draw_walk(
                    cx,
                    scope,
                    Walk {
                        abs_pos: Some(icon_rect.pos),
                        width: Size::Fixed(icon_rect.size.x),
                        height: Size::Fixed(icon_rect.size.y),
                        ..Walk::default()
                    },
                );
            }
        } else {
            self.left_arrow = Rect::default();
            self.right_arrow = Rect::default();
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl FabOverflowTabStripRef {
    pub fn set_tabs(&self, cx: &mut Cx, items: Vec<FabOverflowTab>, active: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_tabs(cx, items, active);
        }
    }

    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FabOverflowTabAction::Selected(index) = item.cast() {
                return Some(index);
            }
        }
        None
    }
}

#[cfg(test)]
mod overflow_tab_tests {
    use super::*;

    #[test]
    fn active_tab_is_revealed_with_minimal_scroll() {
        assert_eq!(reveal_tab(0.0, 100.0, 120.0, 170.0, 200.0), 70.0);
        assert_eq!(reveal_tab(90.0, 100.0, 20.0, 70.0, 200.0), 20.0);
        assert_eq!(reveal_tab(50.0, 100.0, 60.0, 120.0, 200.0), 50.0);
    }

    #[test]
    fn tab_widths_are_bounded_for_overflow_layout() {
        assert_eq!(overflow_tab_width("A"), TAB_MIN_WIDTH);
        assert_eq!(overflow_tab_width(&"A".repeat(100)), TAB_MAX_WIDTH);
    }
}

/// Fold a `FabPanel` when its header is clicked, and turn the chevron with
/// it. Every panel in the app goes through here so the gesture, the animation
/// and the glyph can never disagree.
///
/// `panel` is the `FabPanel` (a `FoldHeader`). Its header lives one level in,
/// at `<panel>.hdr`, because `FoldHeader` flattens its header/body children.
pub fn fold_panel_clicked(view: &View, cx: &mut Cx, actions: &Actions, panel: &[LiveId]) -> bool {
    let mut hdr = panel.to_vec();
    hdr.push(live_id!(hdr));
    if view.view(cx, &hdr).finger_up(actions).is_none() {
        return false;
    }
    let fold = view.fold_header(cx, panel);
    let open = fold.is_open(cx);
    fold.set_is_open(cx, !open, Animate::Yes);
    set_panel_chevron(view, cx, panel, !open);
    true
}

/// Point a panel's chevron down (open) or right (closed).
pub fn set_panel_chevron(view: &View, cx: &mut Cx, panel: &[LiveId], open: bool) {
    let mut path = panel.to_vec();
    path.push(live_id!(hdr));
    path.push(live_id!(tri_slot));
    path.push(live_id!(tri));
    let mut tri = view.widget(cx, &path);
    if tri.is_empty() {
        return;
    }
    let a: f32 = if open {
        0.0
    } else {
        -std::f32::consts::FRAC_PI_2
    };
    script_apply_eval!(cx, tri, {
        draw_icon +: { rotation_angle: #(a) }
    });
}

#[cfg(test)]
mod header_overflow_tests {
    use super::{clamp_header_pan, clipped_header_controls, HeaderControlSpan};

    #[test]
    fn reports_every_partly_or_fully_clipped_control() {
        let spans = [
            HeaderControlSpan {
                start: 0.0,
                end: 40.0,
            },
            HeaderControlSpan {
                start: 44.0,
                end: 94.0,
            },
            HeaderControlSpan {
                start: 98.0,
                end: 128.0,
            },
        ];

        assert_eq!(clipped_header_controls(&spans, 80.0, 0.0), vec![1, 2]);
        assert_eq!(clipped_header_controls(&spans, 80.0, 48.0), vec![0, 1]);
        assert!(clipped_header_controls(&spans, 128.0, 0.0).is_empty());
    }

    #[test]
    fn clamps_pan_to_the_scrollable_extent() {
        assert_eq!(clamp_header_pan(-12.0, 180.0, 100.0), 0.0);
        assert_eq!(clamp_header_pan(35.0, 180.0, 100.0), 35.0);
        assert_eq!(clamp_header_pan(120.0, 180.0, 100.0), 80.0);
        assert_eq!(clamp_header_pan(20.0, 80.0, 100.0), 0.0);
    }

    #[test]
    fn narrow_sun_header_overflows_time_and_haze_but_wide_header_does_not() {
        let sun_controls = [
            HeaderControlSpan {
                start: 180.0,
                end: 250.0,
            },
            HeaderControlSpan {
                start: 254.0,
                end: 320.0,
            },
        ];
        let header_indices = [6, 7];
        let overflow = |width| {
            clipped_header_controls(&sun_controls, width, 0.0)
                .into_iter()
                .map(|index| header_indices[index])
                .collect::<Vec<_>>()
        };

        assert_eq!(overflow(230.0), vec![6, 7]);
        assert!(overflow(320.0).is_empty());
    }
}
