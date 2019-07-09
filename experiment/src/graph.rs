use render::*;
use widget::*;
use serde::*;
use serde::ser::{SerializeStruct};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct GraphState {
    pub nodes: HashMap<Uuid, GraphNode>,
    pub edges: HashMap<Uuid, GraphEdge>,
}

impl GraphState {
    pub fn get_port_rect(&self, addr: &GraphNodePortAddress) -> Option<Rect> {
        match self.nodes.get(&addr.node) {
            Some(node) => {
                match node.get_port_by_address(&addr) {
                    Some(port) => {
                        return Some(port.aabb.clone());
                    },
                    _ => ()
                }
            },
            _ => ()
        };
        None
    }

    pub fn add_edge(&mut self, src: GraphNodePortAddress, dest: GraphNodePortAddress) -> Uuid {
        let edge = GraphEdge {
            start: src.clone(),
            end: dest.clone(),
            id: Uuid::new_v4(),
            animator: Animator::new(Anim::empty())
        };
        let id = edge.id.clone();
        self.edges.insert(id, edge);
        return id;
    }

}

#[derive(Clone)]
pub struct Graph {
    pub state: GraphState,
    pub graph_nodes: Vec<GraphNode>,
    pub graph_edges: Vec<GraphEdge>,

    pub temp_graph_edge: Option<TempGraphEdge>,

    pub graph_view: View<NoScrollBar>,
    pub animator: Animator,
    pub state_file_read: FileRead,
    pub state_file: String,
    pub graph_node_bg: Quad,
    pub graph_node_port_bg: Quad,
    pub graph_edge_end_bg: Quad,
    pub graph_edge_connector_bg: Quad,
}

impl Serialize for Graph {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // 3 is the number of fields in the struct.
        let mut state = serializer.serialize_struct("Graph", 2)?;
        state.serialize_field("nodes", &self.state.nodes)?;
        state.serialize_field("edges", &self.state.edges)?;
        state.end()
    }
}

impl Style for Graph {
    fn style(cx: &mut Cx) -> Self {
        Self {
            state: Default::default(),

            temp_graph_edge: None,
            //state: Default::default(),
            graph_view: View {
                ..Style::style(cx)
            },
            graph_node_bg: Quad {
                color: color("#DDD"),
                shader: cx.add_shader(Self::def_graph_node_bg_shader(), "GraphNode.node_bg"),
                ..Quad::style(cx)
            },
            graph_node_port_bg: Quad {
                color: color("#BBB"),
                shader: cx.add_shader(Self::def_graph_node_port_bg_shader(), "GraphNodePort.node_bg"),
                ..Quad::style(cx)
            },
            graph_edge_end_bg: Quad {
                color: color("#F00"),
                shader: cx.add_shader(Self::def_graph_edge_end_bg_shader(), "GraphEdgeEnd.node_bg"),
                ..Quad::style(cx)
            },
            graph_edge_connector_bg: Quad {
                color: color("#0F0"),
                shader: cx.add_shader(
                    Self::def_graph_edge_connector_bg_shader(),
                    "GraphEdgeConnector.node_bg",
                ),
                ..Quad::style(cx)
            },
            state_file_read: FileRead::default(),
            state_file: format!("{}graph_state.json", "./".to_string()).to_string(),
            animator: Animator::new(Anim::empty()),
            //render_state: Default::default(),

            graph_nodes: Vec::new(),
            graph_edges: Vec::new(),
        }
    }
}

impl Graph {
    pub fn def_graph_node_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_box(2.0, 2.0, w - 4., h - 4., 3.);
                return df_stroke(color, 2.);
            }
        }))
    }

    pub fn def_graph_node_port_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                df_circle(0. + w/2.0, 0. + w/2.0, w / 2.0 - 2.);
                return df_fill(color);
            }
        }))
    }

    pub fn def_graph_edge_end_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));
                //df_circle(0. + w / 2.0, 0. + w / 2.0, w / 2.0 - 2.);
                df_box(2.0, 2.0, w - 4., h - 4., 3.);
                return df_fill(color);
            }
        }))
    }

    pub fn def_graph_edge_connector_bg_shader() -> ShaderGen {
        Quad::def_quad_shader().compose(shader_ast!({
            let start: vec2<Instance>;
            let end: vec2<Instance>;

            fn pixel() -> vec4 {
                df_viewport(pos * vec2(w, h));

                df_move_to(start.x, start.y);
                df_line_to(end.x, end.y);
                df_stroke(color, 2.);

                df_circle(end.x, end.y, 6.);
                df_fill(color("#00F"));

                df_circle(start.x, start.y, 6.);
                return df_fill(color("#F00"));
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
                    let node1 = GraphNode {
                        aabb: Rect {
                            x: 100.0,
                            y: 300.0,
                            w: 100.0,
                            h: 50.0,
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

                    let src_addr = node1.get_port_address(PortDirection::Output, 0);
                    self.add_node(node1);

                    let node2 = GraphNode {
                        aabb: Rect {
                            x: 300.0,
                            y: 300.0,
                            w: 100.0,
                            h: 50.0,
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

                    let dest_addr = node2.get_port_address(PortDirection::Input, 1);
                    self.add_node(node2);
                    self.state.add_edge(src_addr.unwrap(), dest_addr.unwrap());
                }
            },
            _ => ()
        }

        let mut save = false;
        let mut new_edge: Option<GraphEdge> = None;
        let mut skip: Option<Uuid> = None;
        match &mut self.temp_graph_edge {
            Some(edge) => {
                skip = Some(edge.start.port.clone());
            },
            _ => ()
        }
        for (node_id, node) in &mut self.state.nodes {
            match node.handle_graph_node(cx, event, &skip) {
                GraphNodeEvent::DragMove {..} => {
                    self.graph_view.redraw_view_area(cx);
                    save = true;
                },
                GraphNodeEvent::PortDragMove {port_id, port_dir, fe } => {
                    self.graph_view.redraw_view_area(cx);
                    match &mut self.temp_graph_edge {
                        Some(edge) => {
                            edge.end = fe.abs.clone();
                        },
                        None => {
                            self.temp_graph_edge = Some(TempGraphEdge {
                                start: GraphNodePortAddress{
                                    node: node_id.clone(),
                                    port: port_id,
                                    dir: port_dir
                                },
                                end: fe.abs.clone(),
                                ..Default::default()
                            });
                        }
                    }
                },
                GraphNodeEvent::PortDropHit {port_id, port_dir} => {
                    match &mut self.temp_graph_edge {
                        Some(edge) => {
                            edge.target = Some(GraphNodePortAddress{
                                node: node_id.clone(),
                                port: port_id,
                                dir: port_dir
                            });
                        },
                        _ => ()
                    }
                },
                GraphNodeEvent::PortDropMiss => {
                    match &mut self.temp_graph_edge {
                        Some(edge) => {
                            edge.target = None;
                        },
                        _ => ()
                    }
                },
                GraphNodeEvent::PortDrop => {

                    self.graph_view.redraw_view_area(cx);
                    match &mut self.temp_graph_edge {
                        Some(edge) => {
                            match &edge.target {
                                Some(target) => {
                                    if edge.start.dir == target.dir {
                                        self.temp_graph_edge = None;
                                        return;
                                    }

                                    if edge.start.node == target.node {
                                        self.temp_graph_edge = None;
                                        return;
                                    }
                                    
                                    match edge.start.dir {
                                        PortDirection::Input => {

                                            let edge = GraphEdge {
                                                start: target.clone(),
                                                end: edge.start.clone(),
                                                id: Uuid::new_v4(),
                                                animator: Animator::new(Anim::empty())
                                            };
                                            let id = edge.id.clone();
                                            self.state.edges.insert(id, edge);
                                        },
                                        PortDirection::Output => {
                                            let edge = GraphEdge {
                                                start: edge.start.clone(),
                                                end: target.clone(),
                                                id: Uuid::new_v4(),
                                                animator: Animator::new(Anim::empty())
                                            };
                                            let id = edge.id.clone();
                                            self.state.edges.insert(id, edge);
                                        },
                                        _ => ()
                                    }
                                },
                                None => {
                                    self.temp_graph_edge = None;
                                }
                            }
                        },
                        _ => ()
                    }
                },
                _ => ()
            }
        }

        match event {
            Event::FingerUp(fe) => {
                self.temp_graph_edge = None;
                self.graph_view.redraw_view_area(cx);
            },
            _ => ()
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

        for (id, node) in &mut self.state.nodes {
            node.draw_graph_node(cx, &mut self.graph_node_bg, &mut self.graph_node_port_bg);
        }

        // Only fully set edges
        let mut edge_list = vec![];
        for (id, edge) in &self.state.edges {
            match self.state.get_port_rect(&edge.start) {
                Some(start) => {
                    match self.state.get_port_rect(&edge.end) {
                        Some(end) => {
                            edge_list.push((
                                Vec2{x: start.x, y: start.y},
                                Vec2{x: end.x, y: end.y}
                            ));
                        },
                        _ => ()
                    }
                },
                _ => ()
            }
        }

        // FIXME: this assumes nothing changed between the vec construction and now.
        let mut loc = 0;
        for (id, edge) in &mut self.state.edges {
            if (loc >= edge_list.len()) {
                break;
            }
            let (start, end) = edge_list[loc];
            edge.draw_graph_edge(cx,
                start,
                end,
                &mut self.graph_edge_end_bg,
                &mut self.graph_edge_connector_bg
            );
            loc = loc + 1;
        }

        match &mut self.temp_graph_edge {
            Some(edge) => {
                match self.state.get_port_rect(&edge.start) {
                    Some(start) => {
                        edge.draw_graph_edge(cx,
                            Vec2{x: start.x, y: start.y},
                            edge.start.dir.clone(),
                            edge.end,
                            &mut self.graph_edge_end_bg,
                            &mut self.graph_edge_connector_bg
                        );
                    }
                    _ => ()
                }
            },
            _ => ()
        }

        // TODO: render partial edges (e.g., anchored to one port and still dragging)

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
