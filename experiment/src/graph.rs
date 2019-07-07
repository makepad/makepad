use render::*;
use widget::*;
use serde::*;
use serde::ser::{SerializeStruct};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct GraphState {
    pub nodes: HashMap<Uuid, GraphNode>,
//    pub ports: HashMap<Uuid, GraphNodePort>,
//    pub edges: HashMap<Uuid, GraphEdge>,
}

#[derive(Clone)]
pub struct Graph {
    pub state: GraphState,
    pub graph_nodes: Vec<GraphNode>,
    pub graph_edges: Vec<GraphEdge>,
    pub graph_view: View<NoScrollBar>,
    pub animator: Animator,
    pub state_file_read: FileRead,
    pub state_file: String,
    pub graph_node_bg: Quad,
}

impl Serialize for Graph {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 3 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("Graph", 2)?;
        state.serialize_field("nodes", &self.state.nodes)?;
        //state.serialize_field("edges", &self.graph_edges)?;
        state.end()
    }
}

impl Style for Graph {
    fn style(cx: &mut Cx) -> Self {
        Self {
            state: Default::default(),

            //state: Default::default(),
            graph_view: View {
                ..Style::style(cx)
            },
            graph_node_bg: Quad {
                color: color("#DDD"),
                shader: cx.add_shader(Self::def_node_bg_shader(), "GraphNode.node_bg"),
                ..Quad::style(cx)
            },
            state_file_read: FileRead::default(),
            state_file: format!("{}graph_state.json", "./".to_string()).to_string(),
            animator: Animator::new(Anim::empty()),
            //render_state: Default::default(),

            graph_nodes: Vec::new(),
            graph_edges: vec![
                GraphEdge {
                    start: Vec2{x: 300., y: 100.},
                    end: Vec2{ x: 500., y: 150.},
                    ..Style::style(cx)
                }
            ],
        }
    }
}

impl Graph {
    pub fn def_node_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(2.0, 2.0, w - 4., h - 4., 3.);
                return df_stroke(color, 2.);
            }
        }))
    }

    pub fn add_node(&mut self, node: GraphNode) -> Uuid {
        let id = node.id.clone();
        self.state.nodes.insert(node.id, node);
        return id;
    }


    // TODO: return graph event?
    pub fn handle_graph(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                self.state_file_read = cx.file_read(&self.state_file[..]);
            },
            Event::FileRead(fr) => {


                if let Some(utf8_data) = self.state_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(state) = serde_json::from_str(&utf8_data) {
                            println!("READ FILE");
                            self.state = state;
                         }
                     }  else {
                        println!("DOING DEFAULT graph state");
                        // self.state = GraphState{
                        //     nodes: HashMap::new(),
                        //     edges: HashMap::new(),
                        //     ports: HashMap::new(),
                        // };

                        //self.add_node(cx);
                    }
                    cx.redraw_child_area(Area::All);
                } else {
                    let node = GraphNode {
                        layout: Layout {
                            abs_origin: Some(Vec2{x: 100.0, y: 300.0}),
                            width: Bounds::Fix(100.0),
                            height: Bounds::Fix(50.0),
                            ..Layout::default()
                        },
                        inputs: vec![
                            GraphNodePort{
                                ..Style::style(cx)
                            },
                            GraphNodePort{
                                ..Style::style(cx)
                            },
                        ],
                        outputs: vec![
                            GraphNodePort{
                                ..Style::style(cx)
                            },
                            GraphNodePort{
                                ..Style::style(cx)
                            },
                        ],
                        ..Style::style(cx)
                    };

                    self.add_node(node);
                }
            },
            _ => ()
        }

        let mut save = false;
        for (id, node) in &mut self.state.nodes {
            match node.handle_graph_node(cx, event) {
                GraphNodeEvent::DragMove {..} => {
                    self.graph_view.redraw_view_area(cx);
                    save = true;
                },
                _ => ()
            }
        }

        if save {
            let json = serde_json::to_string_pretty(&self).unwrap();
            cx.file_write(&self.state_file[..], json.as_bytes());
        }
    }


    pub fn draw_graph(&mut self, cx: &mut Cx) {
        if let Err(()) = self.graph_view.begin_view(cx, Layout::default()){
            return
        }

        let l = self.graph_nodes.len();
        for (id, node) in &mut self.state.nodes {
            // TODO: build layout off of current state
            let inst = self.graph_node_bg.begin_quad(cx, &node.layout);

            node.draw_graph_node(cx);
            node.animator.update_area_refs(cx, inst.clone().into_area());
            self.graph_node_bg.end_quad(cx, &inst);
        }

        for edge in &mut self.graph_edges {
            edge.draw_graph_edge(cx);
        }

        self.graph_view.end_view(cx);
    }
}

/*
    how do we walk from a terminal node to collect deps?
    nodes = getTerminalNodes()
    for node in nodes {
        rec(node)
    }

    fn rec(node) {
        // collect dependency + build execution plan (maybe a LIFO queue?)

        for edge in node.state.inputs {
            prev_node = graph.state.edges[edge]
            req(prev_node)
        }
    }

*/
