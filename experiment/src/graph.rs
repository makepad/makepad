use render::*;
use widget::*;
use serde::*;
use std::collections::HashMap;

// #[derive(Default, Clone, Serialize, Deserialize)]
// pub struct GraphState {
//     pub nodes: HashMap<Uuid, GraphNodeState>,
//     pub ports: HashMap<Uuid, GraphNodePortState>,
//     pub edges: HashMap<Uuid, GraphEdgeState>,
// }
//
// #[derive(Default, Clone)]
// pub struct GraphRenderState {
//     pub nodes: HashMap<Uuid, GraphNode>,
//     pub ports: HashMap<Uuid, GraphNodePort>,
//     pub edges: HashMap<Uuid, GraphEdge>,
// }

#[derive(Clone)]
pub struct Graph {
    //pub state: GraphState,
    pub graph_nodes: Vec<GraphNode>,
    pub graph_edges: Vec<GraphEdge>,

    pub graph_view: View<NoScrollBar>,
    pub animator: Animator,
    pub state_file_read: FileRead,
    pub node_bg: Quad,
    //pub render_state: GraphRenderState,
}

impl Style for Graph {
    fn style(cx: &mut Cx) -> Self {
        Self {
            //state: Default::default(),
            graph_view: View {
                ..Style::style(cx)
            },
            state_file_read: FileRead::default(),
            node_bg: Quad {
                color: color("#DDD"),
                ..Quad::style(cx)
            },
            animator: Animator::new(Anim::empty()),
            //render_state: Default::default(),

            graph_nodes: vec![
                GraphNode {
                    node_bg_layout: Layout {
                        abs_origin: Some(Vec2{x: 100.0, y: 100.0}),
                        width: Bounds::Fix(100.0),
                        height: Bounds::Fix(50.0),
                        ..Default::default()
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
                },
                GraphNode {
                    node_bg_layout: Layout {
                        abs_origin: Some(Vec2{x: 100.0, y: 200.0}),
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
                },
                GraphNode {
                    node_bg_layout: Layout {
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
                },
            ],
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

    // TODO: return graph event?
    pub fn handle_graph(&mut self, cx: &mut Cx, event: &mut Event) {

        for node in &mut self.graph_nodes {
            match node.handle_graph_node(cx, event) {
                GraphNodeEvent::DragMove {..} => {
                    self.graph_view.redraw_view_area(cx);
                },
                _ => ()
            }
        }

        match event {
            Event::Construct => {
                println!("CONSTRUCT");
                // self.app_global.handle_construct(cx);
                self.state_file_read = cx.file_read(&format!("{}graph_state.json", "./".to_string()));
            },
            Event::FileRead(fr) => {
                println!("READ FILE");
                if let Some(utf8_data) = self.state_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        // if let Ok(state) = serde_json::from_str(&utf8_data) {
                        //     //self.state = state;
                        //  }
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
                }
            },
            _ => ()
        }
    }


    pub fn draw_graph(&mut self, cx: &mut Cx) {
        if let Err(()) = self.graph_view.begin_view(cx, Layout::default()){
            return
        }

        for node in &mut self.graph_nodes {
            node.draw_graph_node(cx);
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
