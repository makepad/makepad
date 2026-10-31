//! `Choose`: a run-time choice gate over candidates.
//!
//! With a `pick` index (literal, wired, or set for the run) it takes that
//! candidate at once. Without one it parks the run like an `Ask`: the
//! scheduler emits `NodeWaiting` with the candidate indices as `options`,
//! and the caller answers by setting the node's `pick` port while it waits
//! (see [`choice_answer`]). The answer is an index into `from`, checked
//! against the candidates; a timeout fails the node (`on_fail: @skip` then
//! skips it).

use super::{param, Executor, Poll};
use crate::{Literal, Node, PortValue, Value};
use std::time::{Duration, Instant};

/// The port a caller writes to answer a waiting Choose.
pub const PICK_PORT: &str = "pick";

/// The value that answers a waiting Choose with candidate `index`.
pub fn choice_answer(index: usize) -> Value {
    Value::json(index.to_string())
}

/// A pick as an index: a JSON integer, or text holding one.
pub fn choice_index(value: &Value) -> Result<usize, String> {
    let text = value.as_text()?.trim();
    let text = text.strip_prefix('"').and_then(|text| text.strip_suffix('"')).unwrap_or(text);
    text.parse::<usize>()
        .map_err(|_| format!("a pick is a candidate index (0, 1, ...), not `{text}`"))
}

#[derive(Default)]
pub struct ChooseExecutor {
    node: String,
    port: String,
    candidates: Vec<Value>,
    picked: Option<usize>,
    deadline: Option<Instant>,
    cancelled: bool,
}

impl ChooseExecutor {
    /// Take the candidates; `Ok(true)` when the node must wait for a pick.
    pub fn begin(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<bool, String> {
        self.node = node.id.clone();
        self.port = node
            .outputs
            .first()
            .map(|port| port.name.clone())
            .ok_or_else(|| format!("Choose `{}` has no output", node.id))?;
        self.candidates = inputs
            .iter()
            .find_map(|(port, value)| (port == "from").then(|| value.values().to_vec()))
            .unwrap_or_default();
        if self.candidates.is_empty() {
            return Err(format!("Choose `{}` has no candidates to choose from", node.id));
        }
        self.deadline = match param(node, "timeout") {
            Some(Literal::Num(seconds)) if *seconds > 0.0 => Some(Instant::now() + Duration::from_secs_f64(*seconds)),
            _ => None,
        };
        let pick = inputs
            .iter()
            .find_map(|(port, value)| (port == PICK_PORT).then(|| value.one()).flatten())
            .filter(|value| value.as_text().is_ok_and(|text| text.trim() != "null"));
        match pick {
            Some(value) => {
                self.picked = Some(self.check(choice_index(value)?)?);
                Ok(false)
            }
            None => Ok(true),
        }
    }

    /// The indices a caller may answer with, for `NodeWaiting.options`.
    pub fn options(&self) -> Vec<Literal> {
        (0..self.candidates.len()).map(|index| Literal::Num(index as f64)).collect()
    }

    pub fn answer(&mut self, node: &str, value: Value) -> Result<bool, String> {
        if self.node != node || self.picked.is_some() {
            return Ok(false);
        }
        self.picked = Some(self.check(choice_index(&value)?)?);
        Ok(true)
    }

    fn check(&self, index: usize) -> Result<usize, String> {
        if index < self.candidates.len() {
            Ok(index)
        } else {
            Err(format!(
                "Choose `{}` has {} candidates; there is no candidate {index}",
                self.node,
                self.candidates.len()
            ))
        }
    }
}

impl Executor for ChooseExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        self.begin(node, inputs).map(|_| ())
    }

    fn poll(&mut self) -> Poll {
        if self.cancelled {
            return Poll::Failed("cancelled".to_string());
        }
        if let Some(index) = self.picked.take() {
            let value = self.candidates[index].clone();
            return Poll::Done(vec![
                (self.port.clone(), PortValue::One(value)),
                ("index".to_string(), PortValue::One(choice_answer(index))),
            ]);
        }
        if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Poll::Failed("timeout".to_string());
        }
        Poll::Pending
    }

    fn cancel(&mut self) {
        self.cancelled = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PortType, Port, Sequence};

    fn choose(timeout: f64) -> Node {
        Node {
            id: "pick".into(),
            kind: "choose".into(),
            type_name: "Choose".into(),
            params: vec![("timeout".into(), Literal::Num(timeout))],
            inputs: Vec::new(),
            outputs: vec![Port { name: "image".into(), ty: PortType::Image, many: None }],
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

    fn candidates(count: usize) -> (String, PortValue) {
        let values = (0..count)
            .map(|index| Value::media(PortType::Image, "image/png", vec![index as u8; 4]))
            .collect();
        ("from".to_string(), PortValue::Seq(Sequence { ty: PortType::Image, values }))
    }

    #[test]
    fn waits_for_an_answer_and_takes_that_candidate() {
        let mut gate = ChooseExecutor::default();
        assert!(gate.begin(&choose(0.0), &[candidates(3)]).unwrap());
        assert_eq!(gate.options(), vec![Literal::Num(0.0), Literal::Num(1.0), Literal::Num(2.0)]);
        assert!(matches!(gate.poll(), Poll::Pending));
        assert!(gate.answer("other", choice_answer(1)).is_ok_and(|taken| !taken));
        assert!(gate.answer("pick", choice_answer(7)).is_err());
        assert!(gate.answer("pick", Value::text("2")).unwrap());
        let Poll::Done(outputs) = gate.poll() else { panic!("not done") };
        assert_eq!(outputs[0].1.one().unwrap().bytes.as_ref(), &[2, 2, 2, 2]);
        assert_eq!(outputs[1].1.one().unwrap().as_text().unwrap(), "2");
    }

    #[test]
    fn a_given_pick_chooses_without_waiting() {
        let mut gate = ChooseExecutor::default();
        let pick = (PICK_PORT.to_string(), PortValue::One(choice_answer(0)));
        assert!(!gate.begin(&choose(0.0), &[candidates(2), pick]).unwrap());
        assert!(matches!(gate.poll(), Poll::Done(_)));

        let mut gate = ChooseExecutor::default();
        let pick = (PICK_PORT.to_string(), PortValue::One(choice_answer(5)));
        assert!(gate.begin(&choose(0.0), &[candidates(2), pick]).is_err());
    }

    #[test]
    fn no_candidates_and_timeouts_fail() {
        let mut gate = ChooseExecutor::default();
        assert!(gate.begin(&choose(0.0), &[candidates(0)]).is_err());
        let mut gate = ChooseExecutor::default();
        assert!(gate.begin(&choose(0.001), &[candidates(1)]).unwrap());
        std::thread::sleep(Duration::from_millis(5));
        assert!(matches!(gate.poll(), Poll::Failed(error) if error == "timeout"));
    }
}
