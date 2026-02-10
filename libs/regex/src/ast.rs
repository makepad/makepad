use {
    super::{char_class::CharClass, prog::Pred},
    std::mem,
};

#[derive(Clone, Debug)]
pub enum Op {
    Empty,
    Cap(usize),
    Alt,
    Cat,
    Ques(bool),
    Star(bool),
    Plus(bool),
    Assert(Pred),
    Char(char),
    CharClass(CharClass),
}

#[derive(Debug)]
pub struct Builder {
    ops: Vec<Op>,
    tmp_ops: Vec<Op>,
    start_stack: Vec<usize>,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            tmp_ops: Vec::new(),
            start_stack: Vec::new(),
        }
    }

    pub fn stack_depth(&self) -> usize {
        self.start_stack.len()
    }

    pub fn empty(&mut self) {
        let start = self.ops.len();
        self.ops.push(Op::Empty);
        self.start_stack.push(start);
    }

    pub fn cap(&mut self, index: usize) {
        self.ops.push(Op::Cap(index));
    }

    pub fn alt(&mut self) {
        self.ops.push(Op::Alt);
        self.start_stack.pop().unwrap();
    }

    pub fn cat(&mut self) {
        self.ops.push(Op::Cat);
        self.start_stack.pop().unwrap();
    }

    pub fn ques(&mut self, greedy: bool) {
        self.ops.push(Op::Ques(greedy));
    }

    pub fn star(&mut self, greedy: bool) {
        self.ops.push(Op::Star(greedy));
    }

    pub fn plus(&mut self, greedy: bool) {
        self.ops.push(Op::Plus(greedy));
    }

    pub fn rep(&mut self, min: u32, max: Option<u32>, greedy: bool) {
        let start = self.start_stack.last().unwrap();
        self.tmp_ops.extend(self.ops.drain(start..));
        match max {
            Some(max) => {
                if min == 0 && max == 0 {
                    self.ops.push(Op::Empty);
                } else if min == 1 && max == 1 {
                    self.ops.extend(self.tmp_ops.iter().cloned());
                } else {
                    if min > 0 {
                        self.ops.extend(self.tmp_ops.iter().cloned());
                        for _ in 1..min {
                            self.ops.extend(self.tmp_ops.iter().cloned());
                            self.ops.push(Op::Cat);
                        }
                    }
                    if max > min {
                        for _ in min..max {
                            self.ops.extend(self.tmp_ops.iter().cloned());
                        }
                        for _ in min + 1..max {
                            self.ops.push(Op::Ques(greedy));
                            self.ops.push(Op::Cat);
                        }
                        self.ops.push(Op::Ques(greedy));
                        if min > 0 {
                            self.ops.push(Op::Cat);
                        }
                    }
                }
            }
            None => {
                if min == 0 {
                    self.ops.extend(self.tmp_ops.iter().cloned());
                    self.ops.push(Op::Star(greedy));
                } else if min == 1 {
                    self.ops.extend(self.tmp_ops.iter().cloned());
                    self.ops.push(Op::Plus(greedy));
                } else {
                    self.ops.extend(self.tmp_ops.iter().cloned());
                    for _ in 1..(min - 1) {
                        self.ops.extend(self.tmp_ops.iter().cloned());
                        self.ops.push(Op::Cat);
                    }
                    self.ops.extend(self.tmp_ops.iter().cloned());
                    self.ops.push(Op::Plus(greedy));
                    self.ops.push(Op::Cat);
                }
            }
        }
        self.tmp_ops.clear();
    }

    pub fn assert(&mut self, pred: Pred) {
        let start = self.ops.len();
        self.ops.push(Op::Assert(pred));
        self.start_stack.push(start);
    }

    pub fn char(&mut self, c: char) {
        let start = self.ops.len();
        self.ops.push(Op::Char(c));
        self.start_stack.push(start);
    }

    pub fn char_class(&mut self, class: CharClass) {
        let start = self.ops.len();
        self.ops.push(Op::CharClass(class));
        self.start_stack.push(start);
    }

    pub fn build(&mut self) -> Vec<Op> {
        mem::replace(&mut self.ops, Vec::new())
    }
}
