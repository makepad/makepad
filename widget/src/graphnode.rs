use crate::graphnodeport::*;
use render::*;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use serde::*;
use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub enum GraphNodeEvent {
    None,
    DragMove { fe: FingerMoveEvent },
    DragEnd { fe: FingerUpEvent },
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

#[derive(Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub layout: Layout,
    pub id: Uuid,
    #[serde(skip_serializing, skip_deserializing, default = "build_animator")]
    pub animator: Animator,
    #[serde(skip_serializing, skip_deserializing)]
    pub inputs: Vec<GraphNodePort>,
    #[serde(skip_serializing, skip_deserializing)]
    pub outputs: Vec<GraphNodePort>,
}

fn build_animator() -> Animator {
    Animator::new(Anim::empty())
}

impl Style for GraphNode {
    fn style(cx: &mut Cx) -> Self {
        Self {
            layout: Layout {
                // width: Bounds::Fix(200.0),
                // height: Bounds::Fix(100.0),
                ..Default::default()
            },
            id: Uuid::new_v4(),
            animator: Animator::new(Anim::empty()),
            inputs: vec![],
            outputs: vec![],
        }
    }
}

impl GraphNode {
    pub fn draw_graph_node(&mut self, cx: &mut Cx) {
        // TODO: eliminate all of these hardcoded offsets. maybe there is
        // value in defining sub views for inputs/outputs
        cx.begin_turtle(&Layout::default(), Area::default());
        cx.move_turtle(-10., 2.0);
        for input in &mut self.inputs {
            input.draw(cx);
            cx.move_turtle(-20., 25.);
        }
        cx.end_turtle(Area::default());

        cx.begin_turtle(&Layout::default(), Area::default());
        cx.move_turtle(-10., 2.0);
        for output in &mut self.outputs {
            output.draw(cx);
            cx.move_turtle(-20., 25.);
        }
        cx.end_turtle(Area::default());
    }

    pub fn handle_graph_node(&mut self, cx: &mut Cx, event: &mut Event) -> GraphNodeEvent {
        for input in &mut self.inputs {
            match input.handle(cx, event) {
                GraphNodePortEvent::Handled => {
                    return GraphNodeEvent::None;
                }
                _ => (),
            }
        }

        for output in &mut self.outputs {
            match output.handle(cx, event) {
                GraphNodePortEvent::Handled => {
                    return GraphNodeEvent::None;
                }
                _ => (),
            }
        }

        match event.hits(cx, self.animator.area, HitOpt::default()) {
            Event::Animate(ae) => {
                self.animator
                    .write_area(cx, self.animator.area, "bg.", ae.time);
            }
            Event::FingerUp(fe) => {
                return GraphNodeEvent::DragEnd { fe: fe.clone() };
            }
            Event::FingerMove(fe) => {
                self.layout.abs_origin = Some(Vec2 {
                    x: fe.abs.x - fe.rel_start.x,
                    y: fe.abs.y - fe.rel_start.y,
                });

                return GraphNodeEvent::DragMove { fe: fe.clone() };
            }
            _ => (),
        }
        GraphNodeEvent::None
    }
}
