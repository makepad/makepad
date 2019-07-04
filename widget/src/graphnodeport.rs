use render::*;

#[derive(Clone, PartialEq)]
pub enum GraphNodePortEvent {
    None,
    Handled,
}

#[derive(Clone, PartialEq)]
pub enum PortDirection {
    Input,
    Output,
}

#[derive(Clone)]
pub struct GraphNodePort {
    pub node_bg_layout: Layout,
    pub node_bg: Quad,
    pub animator: Animator,
}

impl Style for GraphNodePort {
    fn style(cx: &mut Cx) -> Self {
        Self {
            node_bg: Quad {
                color: color("#BBB"),
                shader: cx.add_shader(Self::def_node_bg_shader(), "GraphNodePort.node_bg"),
                ..Quad::style(cx)
            },
            node_bg_layout: Layout {
                width: Bounds::Fix(20.0),
                height: Bounds::Fix(20.0),
                ..Default::default()
            },
            animator: Animator::new(Anim::empty()),
        }
    }
}

impl GraphNodePort {

    pub fn def_node_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_circle(0. + w/2.0, 0. + w/2.0, w / 2.0 - 2.);
                return df_fill(color);
            }
        }))
    }

    pub fn draw(&mut self, cx: &mut Cx) {
        let inst = self.node_bg.begin_quad(cx, &self.node_bg_layout);
        self.animator.update_area_refs(cx, inst.clone().into_area());
        self.node_bg.end_quad(cx, &inst);
    }

    pub fn handle(&mut self, cx: &mut Cx, event: &mut Event) -> GraphNodePortEvent {
        match event.hits(cx, self.animator.area, HitOpt::default()) {
            Event::Animate(ae) => {
                self.animator.write_area(cx, self.animator.area, "bg.", ae.time);
            },
            Event::FingerMove(fe) => {
                return GraphNodePortEvent::Handled;
            }
            _ => ()
        }
        GraphNodePortEvent::None
    }
}
