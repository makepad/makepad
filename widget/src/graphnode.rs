use crate::graphnodeport::*;
use render::*;
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

    pub inputs: Vec<GraphNodePort>,
    pub outputs: Vec<GraphNodePort>,

    #[serde(
        skip_serializing,
        skip_deserializing,
        default = "build_default_animator"
    )]
    pub animator: Animator,
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

fn build_default_animator() -> Animator {
    Animator::new(Anim::empty())
}

impl GraphNode {
    pub fn get_port_address(
        &self,
        dir: PortDirection,
        index: usize,
    ) -> Option<GraphNodePortAddress> {
        let port_id: Uuid;

        // TODO: ensure the thing exists before blindly using it.
        match dir {
            PortDirection::Input => port_id = self.inputs[index].id,
            PortDirection::Output => port_id = self.outputs[index].id,
            PortDirection::None => return None,
        }

        Some(GraphNodePortAddress {
            node: self.id.clone(),
            port: port_id,
            dir: dir,
        })
    }

    pub fn get_port_by_address(&self, addr: &GraphNodePortAddress) -> Option<&GraphNodePort> {
        match addr.dir {
            PortDirection::Input => {
                for input in &self.inputs {
                    if input.id == addr.port {
                        return Some(input);
                    }
                }
            }
            PortDirection::Output => {
                for output in &self.outputs {
                    if output.id == addr.port {
                        return Some(output);
                    }
                }
            }
            _ => (),
        }
        None
    }

    pub fn draw_graph_node(&mut self, cx: &mut Cx, bg: &mut Quad, port_bg: &mut Quad) {
        // TODO: build layout off of current state
        let inst = bg.begin_quad(cx, &self.layout);

        // TODO: eliminate all of these hardcoded offsets. maybe there is
        // value in defining sub views for inputs/outputs
        let mut y = 5.0;
        let origin = self.layout.abs_origin.unwrap();
        let size = self.layout.abs_size.unwrap();
        for input in &mut self.inputs {
            let rect = Rect {
                x: origin.x - 10.0,
                y: origin.y + y,
                w: 20.0,
                h: 20.0,
            };
            input.draw(cx, port_bg, rect);
            y += 20.0;
        }

        y = 5.0;
        for output in &mut self.outputs {
            let rect = Rect {
                x: size.x + origin.x - 10.0,
                y: origin.y + y,
                w: 20.0,
                h: 20.0,
            };
            output.draw(cx, port_bg, rect);
            y += 20.0;
        }

        self.animator.update_area_refs(cx, inst.clone().into_area());
        bg.end_quad(cx, &inst);
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
