use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    animator::{Animator, AnimatorImpl, AnimatorAction},
};

script_mod!{
    use mod.prelude.widgets_internal.*
    
    mod.widgets.TabCloseButtonBase = #(TabCloseButton::script_component(vm))
    
    mod.widgets.TabCloseButton = mod.std.set_type_default() do mod.widgets.TabCloseButtonBase{
        height: 10.0
        width: 10.0
        margin: Inset{right: theme.space_2, left: -3.5}
        draw_button +: {
            hover: instance(0.0)
            active: instance(0.0)

            size: uniform(1.0)

            color: uniform(#8)
            color_hover: uniform(#C)
            color_active: uniform(#A)
            
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let mid = self.rect_size / 2.0
                let size = (self.hover * 0.25 + 0.5) * 0.25 * length(self.rect_size) * self.size
                let min = mid - vec2(size size)
                let max = mid + vec2(size size)
                sdf.move_to(min.x, min.y)
                sdf.line_to(max.x, max.y)
                sdf.move_to(min.x, max.y)
                sdf.line_to(max.x, min.y)

                return sdf.stroke(
                    mix(
                        mix(self.color, self.color_hover, self.hover)
                        mix(self.color_active, self.color_hover, self.hover)
                        self.active
                    ) 1.0
                )
            }
        }
        
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward{duration: 0.1}}
                    apply: {
                        draw_button: {hover: 0.0}
                    }
                }
                
                on: AnimatorState{
                    cursor: MouseCursor.Hand
                    from: {all: Snap}
                    apply: {
                        draw_button: {hover: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Animator)]
pub struct TabCloseButton {
    #[source] source: ScriptObjectRef,
    #[live] draw_button: DrawQuad,
    #[apply_default] animator: Animator,
    #[walk] walk: Walk
}

impl TabCloseButton {
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_button.draw_walk(
            cx,
            self.walk
        );
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
    ) -> TabCloseButtonAction {
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_button.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
                return TabCloseButtonAction::HoverIn;
            }
            Hit::FingerHoverOut(_)=>{
                self.animator_play(cx, ids!(hover.off));
                return TabCloseButtonAction::HoverOut;
            }
            // Pressing the tab close button with a primary button/touch
            // or the middle mouse button are both recognized as a close tab action.
            Hit::FingerDown(fe) 
                if fe.is_primary_hit() || fe.mouse_button().is_some_and(|b| b.is_middle()) =>
            {
                return TabCloseButtonAction::WasPressed;
            }
            _ => {}
        }
        TabCloseButtonAction::None
    }
}

pub enum TabCloseButtonAction {
    None,
    WasPressed,
    HoverIn,
    HoverOut,
}
