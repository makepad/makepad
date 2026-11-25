//! Frame delays for paths that meet at a node or at the device outputs.
use relax_api::relax_api;

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AlignNode {
    pub latency: u32,
    /// A source starts its own path at frame zero, even when it has inputs.
    pub source: bool,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AlignEdge {
    pub from: usize,
    pub to: usize,
    /// Extra frames on this connection after alignment.
    pub deliberate: u32,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeTiming {
    pub l_in: u64,
    pub l_out: u64,
}

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeTiming {
    pub delay: u64,
    /// How far the sender trails a source's frame zero. Zero on aligned edges.
    pub lateness: u64,
    pub aligned: bool,
}

#[derive(Debug)]
pub struct Alignment {
    pub nodes: Vec<NodeTiming>,
    pub edges: Vec<EdgeTiming>,
    pub pads: Vec<u64>,
    pub total: u64,
}

#[relax_api(plain)]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignError {
    Cycle(usize),
    UnknownNode(usize),
    FrameOverflow(usize),
}

#[relax_api(plain)]
#[repr(u8)]
enum AlignStatus {
    Complete(u64),
    Failed(AlignError),
}

/// Computes timings in the original node, edge and output orders. Outputs are
/// node indices. Their pads bring every output's input to the same frame.
#[inline(always)]
pub fn align(nodes: &[AlignNode], edges: &[AlignEdge], outputs: &[usize]) -> Result<Alignment, AlignError> {
    let mut timings = vec![NodeTiming::default(); nodes.len()];
    let mut delays = vec![EdgeTiming::default(); edges.len()];
    let mut pads = vec![0; outputs.len()];
    match align_into(nodes, edges, outputs, &mut timings, &mut delays, &mut pads) {
        AlignStatus::Complete(total) => Ok(Alignment { nodes: timings, edges: delays, pads, total }),
        AlignStatus::Failed(error) => Err(error),
    }
}

// All result storage belongs to the caller. Only slices and the status cross.
#[relax_api]
fn align_into(
    nodes: &[AlignNode], edges: &[AlignEdge], outputs: &[usize],
    timings: &mut [NodeTiming], delays: &mut [EdgeTiming], pads: &mut [u64],
) -> AlignStatus {
    let mut inputs = vec![Vec::new(); nodes.len()];
    for edge in edges {
        if edge.from >= nodes.len() { return AlignStatus::Failed(AlignError::UnknownNode(edge.from)); }
        if edge.to >= nodes.len() { return AlignStatus::Failed(AlignError::UnknownNode(edge.to)); }
        inputs[edge.to].push(edge.from);
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Visit { Unseen, Active, Done }
    let mut visit = vec![Visit::Unseen; nodes.len()];
    let mut stack = Vec::new();
    for root in 0..nodes.len() {
        stack.push((root, false));
        while let Some((n, finishing)) = stack.pop() {
            if finishing {
                let l_in = if nodes[n].source { 0 } else {
                    inputs[n].iter().map(|&from| timings[from].l_out).max().unwrap_or(0)
                };
                let Some(l_out) = l_in.checked_add(u64::from(nodes[n].latency)) else {
                    return AlignStatus::Failed(AlignError::FrameOverflow(n));
                };
                timings[n] = NodeTiming { l_in, l_out };
                visit[n] = Visit::Done;
            } else {
                match visit[n] {
                    Visit::Done => continue,
                    Visit::Active => return AlignStatus::Failed(AlignError::Cycle(n)),
                    Visit::Unseen => visit[n] = Visit::Active,
                }
                stack.push((n, true));
                stack.extend(inputs[n].iter().map(|&from| (from, false)));
            }
        }
    }
    for (edge, timing) in edges.iter().zip(delays) {
        let sent = timings[edge.from].l_out;
        let deliberate = u64::from(edge.deliberate);
        *timing = if nodes[edge.to].source {
            EdgeTiming { delay: deliberate, lateness: sent, aligned: false }
        } else {
            let Some(delay) = (timings[edge.to].l_in - sent).checked_add(deliberate) else {
                return AlignStatus::Failed(AlignError::FrameOverflow(edge.to));
            };
            EdgeTiming { delay, lateness: 0, aligned: true }
        };
    }
    let mut total = 0;
    for &output in outputs {
        let Some(timing) = timings.get(output) else { return AlignStatus::Failed(AlignError::UnknownNode(output)); };
        total = total.max(timing.l_in);
    }
    for (&output, pad) in outputs.iter().zip(pads) {
        *pad = total - timings[output].l_in;
    }
    AlignStatus::Complete(total)
}
