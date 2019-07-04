use render::*;
use crate::graphnodeport::*;


#[derive(Clone, PartialEq)]
pub enum GraphNodeEvent {
    None,
    DragMove {fe: FingerMoveEvent},
    DragEnd {fe: FingerUpEvent},
    DragOut,
}

/*
  .....................
  .       A NODE      .
  .....................
 (IN) --        -- (OUT)
  .      \    /       .
 (IN) -- (CORE)       .
  .      /    \       .
 (IN) --        -- (OUT)
  ......................
*/

#[derive(Clone)]
pub struct GraphNode {
    pub node_bg_layout: Layout,
    pub node_bg: Quad,
    pub animator: Animator,
    pub inputs: Vec<GraphNodePort>,
    pub outputs: Vec<GraphNodePort>,
}

impl Style for GraphNode {
    fn style(cx: &mut Cx) -> Self {
        Self {
            node_bg: Quad {
                color: color("#DDD"),
                shader: cx.add_shader(Self::def_node_bg_shader(), "GraphNode.node_bg"),
                ..Quad::style(cx)
            },
            node_bg_layout: Layout {
                width: Bounds::Fix(200.0),
                height: Bounds::Fix(100.0),
                ..Default::default()
            },
            animator: Animator::new(Anim::empty()),
            inputs: vec![],
            outputs: vec![],
        }
    }
}

impl GraphNode {

    pub fn def_node_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(0.0, 0.0, w, h, 2.);
                return df_stroke(color, 2.);
            }
        }))
    }

    pub fn draw_graph_node(&mut self, cx: &mut Cx) {
        let inst = self.node_bg.begin_quad(cx, &self.node_bg_layout);

        // TODO: eliminate all of these hardcoded offsets. maybe there is
        // value in defining sub views for inputs/outputs
        cx.begin_turtle(&Layout::default(), Area::default());
        cx.move_turtle(-10., 0.0);
        for input in &mut self.inputs {
            input.draw(cx);
            cx.move_turtle(-20., 25.);
        }
        cx.end_turtle(Area::default());

        cx.begin_turtle(&Layout::default(), Area::default());
        cx.move_turtle(-10., 0.0);
        for output in &mut self.outputs {
            output.draw(cx);
            cx.move_turtle(-20., 25.);
        }
        cx.end_turtle(Area::default());

        self.animator.update_area_refs(cx, inst.clone().into_area());
        self.node_bg.end_quad(cx, &inst);
    }

    pub fn handle_graph_node(&mut self, cx: &mut Cx, event: &mut Event) -> GraphNodeEvent {
        for input in &mut self.inputs {
            match input.handle(cx, event) {
                GraphNodePortEvent::Handled => {
                    return GraphNodeEvent::None;
                },
                _ => ()
            }
        }

        for output in &mut self.outputs {
            match output.handle(cx, event) {
                GraphNodePortEvent::Handled => {
                    return GraphNodeEvent::None;
                },
                _ => ()
            }
        }

        match event.hits(cx, self.animator.area, HitOpt::default()) {
            Event::Animate(ae) => {
                self.animator.write_area(cx, self.animator.area, "bg.", ae.time);
            },
            Event::FingerUp(fe) => {
                return GraphNodeEvent::DragEnd{
                    fe: fe.clone(),
                };
            }
            Event::FingerMove(fe) => {
                self.node_bg_layout.abs_origin = Some(Vec2{
                    x: fe.abs.x - fe.rel_start.x,
                    y: fe.abs.y - fe.rel_start.y
                });

                return GraphNodeEvent::DragMove{
                    fe: fe.clone(),
                };
            },
            _ => ()
        }
        GraphNodeEvent::None
    }
}
