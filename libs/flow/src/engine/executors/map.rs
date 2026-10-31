//! `Map`: one job node run once per element of a list.
//!
//! The Map's `body` wire names the job (the node it runs, and the port it
//! collects); `bind` names the body parameter or input port each element
//! sets. Elements start in order, at most `concurrency` at once, and the
//! results come back as `items` in element order whatever order the jobs
//! finish in, so a run is deterministic for a given list. A body with
//! `on_fail: @skip` drops the elements it fails on; otherwise the first
//! failure cancels the rest and fails the Map. The scheduler starts each
//! element's executor (the same executors a node of that kind uses); this
//! file owns the order, the bound and the collection.

use super::{param, Poll};
use crate::{Literal, Node, NodeInputValue, PortType, PortValue, Sequence, Value};
use makepad_strict_json::Value as Json;

/// What the Map drives per element: a started executor.
pub trait MapChild {
    fn poll(&mut self) -> Poll;
    fn cancel(&mut self);
}

/// A child start: the element's node and resolved inputs to an executor.
pub type StartChild<'a, C> = dyn FnMut(&Node, &[(String, PortValue)]) -> Result<C, String> + 'a;

struct Running<C> {
    index: usize,
    child: C,
    permille: u16,
}

pub struct MapRun<C> {
    map: String,
    body: Node,
    /// The body output the Map collects.
    port: String,
    ty: PortType,
    bind: String,
    body_inputs: Vec<(String, PortValue)>,
    elements: Vec<Literal>,
    concurrency: usize,
    skip_failures: bool,
    next: usize,
    running: Vec<Running<C>>,
    /// Per element: the collected value and its generation record, or why
    /// it was dropped.
    results: Vec<Option<Result<(Value, Json), String>>>,
    reported: Option<(u16, usize)>,
    cancelled: bool,
}

impl<C: MapChild> MapRun<C> {
    /// `map_inputs` are the Map's own resolved inputs (`over`); `body_inputs`
    /// the body's, resolved as they would be for a run of the body itself.
    pub fn new(
        map: &Node,
        body: &Node,
        map_inputs: &[(String, PortValue)],
        body_inputs: Vec<(String, PortValue)>,
    ) -> Result<Self, String> {
        let port = map
            .inputs
            .iter()
            .find_map(|input| match (&input.value, input.port.as_str()) {
                (NodeInputValue::Edge(edge), "body") => Some(edge.from_port.clone()),
                _ => None,
            })
            .ok_or_else(|| format!("Map `{}` needs `body: node.port()`", map.id))?;
        let items = map
            .outputs
            .first()
            .ok_or_else(|| format!("Map `{}` has no items port", map.id))?;
        let bind = match param(map, "bind") {
            Some(Literal::Id(name) | Literal::Str(name)) => name.clone(),
            _ => return Err(format!("Map `{}` needs `bind`", map.id)),
        };
        let over = map_inputs
            .iter()
            .find_map(|(port, value)| (port == "over").then(|| value.one()).flatten())
            .ok_or_else(|| format!("Map `{}` has no `over` list", map.id))?;
        let elements = list_elements(over).map_err(|error| format!("Map `{}` `over`: {error}", map.id))?;
        if let Some(bound) = items.many {
            if elements.len() > bound as usize {
                return Err(format!(
                    "Map `{}` maps {} elements, over its bound of {bound}; raise `many`",
                    map.id,
                    elements.len()
                ));
            }
        }
        let concurrency = match param(map, "concurrency") {
            Some(Literal::Num(value)) if *value >= 1.0 => *value as usize,
            _ => 1,
        };
        Ok(Self {
            map: map.id.clone(),
            body: body.clone(),
            port,
            ty: items.ty.clone(),
            bind,
            body_inputs,
            results: elements.iter().map(|_| None).collect(),
            elements,
            concurrency,
            skip_failures: body.on_fail == "skip",
            next: 0,
            running: Vec::new(),
            reported: None,
            cancelled: false,
        })
    }

    /// Element `index`'s node and inputs: the body with the element bound.
    pub fn element(&self, index: usize) -> Result<(Node, Vec<(String, PortValue)>), String> {
        let element = &self.elements[index];
        let mut node = self.body.clone();
        node.id = format!("{}[{index}]", self.body.id);
        let mut inputs = self.body_inputs.clone();
        if let Some(input) = node.inputs.iter_mut().find(|input| input.port == self.bind) {
            let value = element_value(&input.ty, element)
                .map_err(|error| format!("element {index} for `{}.{}`: {error}", self.body.id, self.bind))?;
            input.value = NodeInputValue::Literal(Literal::Null);
            inputs.retain(|(port, _)| *port != self.bind);
            inputs.push((self.bind.clone(), PortValue::One(value)));
        } else if let Some(slot) = node.params.iter_mut().find(|(name, _)| *name == self.bind) {
            slot.1 = element.clone();
        } else {
            return Err(format!("`{}` has no parameter or input `{}` to bind", self.body.id, self.bind));
        }
        Ok((node, inputs))
    }

    /// Start what the bound allows, poll what runs, and report. `start` is
    /// `None` only where no executor may be started (a stray poll).
    pub fn step(&mut self, mut start: Option<&mut StartChild<'_, C>>) -> Poll {
        if self.cancelled {
            return Poll::Failed("cancelled".to_string());
        }
        if let Some(start) = start.as_deref_mut() {
            while self.running.len() < self.concurrency && self.next < self.elements.len() {
                let index = self.next;
                self.next += 1;
                let started = self.element(index).and_then(|(node, inputs)| start(&node, &inputs));
                match started {
                    Ok(child) => self.running.push(Running { index, child, permille: 0 }),
                    Err(error) => {
                        if let Some(failed) = self.element_failed(index, error) {
                            return failed;
                        }
                    }
                }
            }
        }
        let mut position = 0;
        while position < self.running.len() {
            match self.running[position].child.poll() {
                Poll::Pending | Poll::Stage { .. } | Poll::Delta { .. } => position += 1,
                Poll::Progress { permille, .. } => {
                    self.running[position].permille = permille.min(999);
                    position += 1;
                }
                Poll::Done(outputs) => {
                    let finished = self.running.remove(position);
                    match collect(&self.port, &outputs) {
                        Ok(collected) => self.results[finished.index] = Some(Ok(collected)),
                        Err(error) => {
                            if let Some(failed) = self.element_failed(finished.index, error) {
                                return failed;
                            }
                        }
                    }
                }
                Poll::Failed(error) => {
                    let finished = self.running.remove(position);
                    if let Some(failed) = self.element_failed(finished.index, error) {
                        return failed;
                    }
                }
            }
        }
        if self.results.iter().all(Option::is_some) {
            return self.finish();
        }
        let settled = self.results.iter().filter(|result| result.is_some()).count();
        let total = self.elements.len().max(1);
        let sum = settled * 1000 + self.running.iter().map(|running| running.permille as usize).sum::<usize>();
        let permille = (sum / total).min(999) as u16;
        if self.reported == Some((permille, settled)) {
            return Poll::Pending;
        }
        self.reported = Some((permille, settled));
        Poll::Progress {
            permille,
            stage: format!("{settled} of {} done, {} running", self.elements.len(), self.running.len()),
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        for running in &mut self.running {
            running.child.cancel();
        }
        self.running.clear();
    }

    /// Record an element's failure: dropped under `on_fail: @skip`, else the
    /// whole Map fails and every running element is cancelled.
    fn element_failed(&mut self, index: usize, error: String) -> Option<Poll> {
        if self.skip_failures {
            self.results[index] = Some(Err(error));
            return None;
        }
        for running in &mut self.running {
            running.child.cancel();
        }
        self.running.clear();
        self.cancelled = true;
        Some(Poll::Failed(format!(
            "Map `{}` element {index} ({}): {error}",
            self.map,
            literal_text(&self.elements[index])
        )))
    }

    /// `items` in element order, and `used`: one record per item (the
    /// element, the seed and model its job reported), aligned with `items`,
    /// plus the dropped elements and why.
    fn finish(&mut self) -> Poll {
        let mut items = Sequence::empty(self.ty.clone());
        let mut used = Vec::new();
        let mut dropped = Vec::new();
        for (index, result) in self.results.iter_mut().enumerate() {
            match result.take() {
                Some(Ok((value, record))) => {
                    items.values.push(value);
                    used.push(record);
                }
                Some(Err(error)) => dropped.push(Json::Obj(vec![
                    ("element".to_string(), crate::values::literal_json(&self.elements[index])),
                    ("error".to_string(), Json::Str(error)),
                ])),
                None => {}
            }
        }
        let used = Json::Obj(vec![
            ("items".to_string(), Json::Arr(used)),
            ("dropped".to_string(), Json::Arr(dropped)),
        ]);
        Poll::Done(vec![
            ("items".to_string(), PortValue::Seq(items)),
            ("used".to_string(), PortValue::One(Value::json(used.to_json()))),
        ])
    }
}

/// The collected port and the job's own record of what it used.
fn collect(port: &str, outputs: &[(String, PortValue)]) -> Result<(Value, Json), String> {
    let one = |name: &str| outputs.iter().find_map(|(port, value)| (port == name).then(|| value.one()).flatten());
    let value = one(port).ok_or_else(|| format!("the body produced no `{port}`"))?.clone();
    let mut record = Vec::new();
    if let Some(seed) = one("seed_used").and_then(|value| value.as_text().ok()).and_then(|text| text.trim().parse::<u64>().ok()) {
        record.push(("seed_used".to_string(), Json::Int(seed.min(i64::MAX as u64) as i64)));
    }
    if let Some(model) = one("model_used").and_then(|value| value.as_text().ok()) {
        record.push(("model_used".to_string(), Json::Str(model.to_string())));
    }
    Ok((value, Json::Obj(record)))
}

/// The elements of a list value.
pub fn list_elements(value: &Value) -> Result<Vec<Literal>, String> {
    let json = makepad_strict_json::parse(&value.bytes).map_err(|error| format!("not a JSON list: {error}"))?;
    match json {
        Json::Arr(items) => Ok(items.iter().map(json_literal).collect()),
        _ => Err("not a list".to_string()),
    }
}

fn json_literal(value: &Json) -> Literal {
    match value {
        Json::Null => Literal::Null,
        Json::Bool(value) => Literal::Bool(*value),
        Json::Int(value) => Literal::Num(*value as f64),
        Json::F64(value) => Literal::Num(*value),
        Json::Str(value) => Literal::Str(value.clone()),
        Json::Arr(items) => Literal::Arr(items.iter().map(json_literal).collect()),
        Json::Obj(fields) => Literal::Obj(fields.iter().map(|(name, value)| (name.clone(), json_literal(value))).collect()),
    }
}

/// An element as a value of an input port's type: a string element as
/// text, anything else as its JSON text; a JSON port takes it as it is.
fn element_value(ty: &PortType, element: &Literal) -> Result<Value, String> {
    match (ty, element) {
        (PortType::Text, Literal::Str(_) | Literal::Id(_)) => Value::from_literal(PortType::Text, element),
        (PortType::Text, _) => Ok(Value::text(literal_text(element))),
        (PortType::Json | PortType::List, _) => Value::from_literal(ty.clone(), element),
        _ => Err(format!("a list element cannot be a {} value", ty.as_str())),
    }
}

fn literal_text(element: &Literal) -> String {
    match element {
        Literal::Str(text) | Literal::Id(text) => text.clone(),
        other => crate::values::literal_json(other).to_json(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EdgeRef, NodeInput, Port};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A fake job: done after `ticks` polls with its seed as the image
    /// bytes, or failing when the seed is negative.
    struct Job {
        seed: f64,
        ticks: usize,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl MapChild for Job {
        fn poll(&mut self) -> Poll {
            if self.seed < 0.0 {
                return Poll::Failed("negative seed".to_string());
            }
            if self.ticks > 0 {
                self.ticks -= 1;
                return Poll::Progress { permille: 500, stage: "painting".to_string() };
            }
            self.log.borrow_mut().push(format!("done {}", self.seed));
            Poll::Done(vec![
                ("image".to_string(), PortValue::One(Value::media(PortType::Image, "image/png", format!("png{}", self.seed).into_bytes()))),
                ("seed_used".to_string(), PortValue::One(Value::json(format!("{}", self.seed)))),
            ])
        }

        fn cancel(&mut self) {
            self.log.borrow_mut().push(format!("cancel {}", self.seed));
        }
    }

    fn node(id: &str, kind: &str, params: Vec<(&str, Literal)>, inputs: Vec<NodeInput>, outputs: Vec<Port>) -> Node {
        Node {
            id: id.into(),
            kind: kind.into(),
            type_name: id.into(),
            params: params.into_iter().map(|(name, value)| (name.to_string(), value)).collect(),
            inputs,
            outputs,
            at: None,
            size: None,
            flip: false,
            loc: Default::default(),
            fn_src: None,
            face_src: None,
            on_fail: "fail".into(),
            label: None,
            domain: None,
            doc: None,
        }
    }

    fn setup(elements: &str, concurrency: f64, skip: bool) -> MapRun<Job> {
        let mut body = node(
            "paint",
            "gen",
            vec![("seed", Literal::Num(0.0))],
            vec![NodeInput { port: "prompt".into(), ty: PortType::Text, many: None, value: NodeInputValue::Literal(Literal::Str("koi".into())) }],
            vec![Port { name: "image".into(), ty: PortType::Image, many: None }],
        );
        if skip {
            body.on_fail = "skip".into();
        }
        let map = node(
            "fan",
            "map",
            vec![("bind", Literal::Id("seed".into())), ("concurrency", Literal::Num(concurrency))],
            vec![NodeInput {
                port: "body".into(),
                ty: PortType::Image,
                many: None,
                value: NodeInputValue::Edge(EdgeRef { from_node: "paint".into(), from_port: "image".into() }),
            }],
            vec![Port { name: "items".into(), ty: PortType::Image, many: Some(8) }],
        );
        let over = vec![("over".to_string(), PortValue::One(Value::list(elements)))];
        MapRun::new(&map, &body, &over, vec![("prompt".to_string(), PortValue::One(Value::text("koi")))]).unwrap()
    }

    fn run(map: &mut MapRun<Job>, log: &Rc<RefCell<Vec<String>>>, ticks: &dyn Fn(f64) -> usize) -> (Poll, usize) {
        let mut most = 0;
        for _ in 0..100 {
            let log = log.clone();
            let mut start = |node: &Node, _inputs: &[(String, PortValue)]| -> Result<Job, String> {
                let seed = match param(node, "seed") { Some(Literal::Num(seed)) => *seed, _ => return Err("no seed".into()) };
                Ok(Job { seed, ticks: ticks(seed), log: log.clone() })
            };
            let poll = map.step(Some(&mut start));
            most = most.max(map.running.len());
            if matches!(poll, Poll::Done(_) | Poll::Failed(_)) {
                return (poll, most);
            }
        }
        panic!("map did not settle");
    }

    fn items(poll: &Poll) -> Vec<String> {
        let Poll::Done(outputs) = poll else { panic!("not done: {poll:?}") };
        outputs[0].1.values().iter().map(|value| String::from_utf8(value.bytes.to_vec()).unwrap()).collect()
    }

    #[test]
    fn items_keep_element_order_whatever_finishes_first() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut map = setup("[3, 1, 2]", 3.0, false);
        // Seed 3 takes longest, so it finishes last but stays first.
        let (poll, _) = run(&mut map, &log, &|seed| seed as usize * 2);
        assert_eq!(items(&poll), ["png3", "png1", "png2"]);
        assert_eq!(log.borrow()[0], "done 1");
        let Poll::Done(outputs) = poll else { unreachable!() };
        let used = outputs[1].1.one().unwrap().as_text().unwrap().to_string();
        assert_eq!(used, r#"{"items":[{"seed_used":3},{"seed_used":1},{"seed_used":2}],"dropped":[]}"#);
    }

    #[test]
    fn concurrency_bounds_the_running_elements_and_queues_the_rest() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut map = setup("[1, 2, 3, 4, 5]", 2.0, false);
        let (poll, most) = run(&mut map, &log, &|_| 3);
        assert_eq!(most, 2);
        assert_eq!(items(&poll).len(), 5);
    }

    #[test]
    fn a_failure_cancels_the_rest_unless_the_body_skips() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut map = setup("[5, -1, 7]", 3.0, false);
        let (poll, _) = run(&mut map, &log, &|_| 4);
        let Poll::Failed(error) = poll else { panic!("expected failure") };
        assert!(error.contains("element 1 (-1)") && error.contains("negative seed"), "{error}");
        assert!(log.borrow().contains(&"cancel 5".to_string()) && log.borrow().contains(&"cancel 7".to_string()));

        let log = Rc::new(RefCell::new(Vec::new()));
        let mut map = setup("[5, -1, 7]", 3.0, true);
        let (poll, _) = run(&mut map, &log, &|_| 1);
        assert_eq!(items(&poll), ["png5", "png7"]);
    }

    #[test]
    fn cancel_reaches_every_running_element() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut map = setup("[1, 2, 3]", 2.0, false);
        let child_log = log.clone();
        let mut start = |node: &Node, _: &[(String, PortValue)]| -> Result<Job, String> {
            let Some(Literal::Num(seed)) = param(node, "seed") else { unreachable!() };
            Ok(Job { seed: *seed, ticks: 10, log: child_log.clone() })
        };
        assert!(matches!(map.step(Some(&mut start)), Poll::Progress { .. }));
        map.cancel();
        assert_eq!(*log.borrow(), ["cancel 1", "cancel 2"]);
        assert!(matches!(map.step(Some(&mut start)), Poll::Failed(_)));
    }

    #[test]
    fn empty_lists_map_to_empty_items_and_long_lists_are_refused() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut map = setup("[]", 2.0, false);
        let (poll, _) = run(&mut map, &log, &|_| 0);
        assert!(items(&poll).is_empty());

        let body = setup("[]", 1.0, false).body;
        let fan = node("fan", "map", vec![("bind", Literal::Id("seed".into()))], vec![NodeInput {
            port: "body".into(), ty: PortType::Image, many: None,
            value: NodeInputValue::Edge(EdgeRef { from_node: "paint".into(), from_port: "image".into() }),
        }], vec![Port { name: "items".into(), ty: PortType::Image, many: Some(2) }]);
        let over = vec![("over".to_string(), PortValue::One(Value::list("[1, 2, 3]")))];
        let error = MapRun::<Job>::new(&fan, &body, &over, Vec::new()).err().unwrap();
        assert!(error.contains("over its bound of 2"), "{error}");
    }

    #[test]
    fn an_element_binds_an_input_port_as_text() {
        let mut map = setup("[\"a heron\", 7]", 1.0, false);
        map.bind = "prompt".into();
        let (node, inputs) = map.element(0).unwrap();
        assert_eq!(node.id, "paint[0]");
        let prompt = inputs.iter().find(|(port, _)| port == "prompt").unwrap().1.one().unwrap();
        assert_eq!(prompt.as_text().unwrap(), "a heron");
        let (_, inputs) = map.element(1).unwrap();
        assert_eq!(inputs.iter().find(|(port, _)| port == "prompt").unwrap().1.one().unwrap().as_text().unwrap(), "7");
    }
}
