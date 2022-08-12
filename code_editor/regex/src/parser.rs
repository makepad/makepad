use crate::{ast::Quant, Ast, str};

pub struct Parser {
    asts: Vec<Ast>,
    groups: Vec<Group>,
}

impl Parser {
    pub(crate) fn new() -> Self {
        Self {
            asts: Vec::new(),
            groups: Vec::new(),
        }
    }

    pub(crate) fn parse(&mut self, string: &str) -> Ast {
        ParseContext {
            asts: &mut self.asts,
            groups: &mut self.groups,
            group: Group::default(),
            string,
            position: 0,
        }
        .parse()
    }
}

struct ParseContext<'a> {
    asts: &'a mut Vec<Ast>,
    groups: &'a mut Vec<Group>,
    group: Group,
    string: &'a str,
    position: usize,
}

impl<'a> ParseContext<'a> {
    fn parse(&mut self) -> Ast {
        loop {
            match self.peek_char() {
                Some('|') => {
                    self.skip_char();
                    self.maybe_push_cat();
                    self.pop_cats();
                    self.group.alt_count += 1;
                }
                Some('?') => {
                    self.skip_char();
                    let ast = self.asts.pop().unwrap();
                    self.asts.push(Ast::Rep(Box::new(ast), Quant::Quest));
                }
                Some('*') => {
                    self.skip_char();
                    let ast = self.asts.pop().unwrap();
                    self.asts.push(Ast::Rep(Box::new(ast), Quant::Star));
                }
                Some('+') => {
                    self.skip_char();
                    let ast = self.asts.pop().unwrap();
                    self.asts.push(Ast::Rep(Box::new(ast), Quant::Plus));
                }
                Some('(') => {
                    self.skip_char();
                    self.push_group();
                }
                Some(')') => {
                    self.skip_char();
                    self.pop_group();
                }
                Some(ch) => {
                    self.skip_char();
                    self.maybe_push_cat();
                    self.asts.push(Ast::Char(ch));
                    self.group.ast_count += 1;
                }
                None => break,
            }
        }
        self.maybe_push_cat();
        self.pop_alts();
        self.asts.pop().unwrap()
    }

    fn peek_char(&self) -> Option<char> {
        self.string[self.position..].chars().next()
    }

    fn skip_char(&mut self) {
        self.position += str::utf8_char_width(self.string.as_bytes()[self.position]);
    }

    fn push_group(&mut self) {
        use std::mem;

        self.maybe_push_cat();
        self.pop_cats();
        let group = mem::replace(&mut self.group, Group::default());
        self.groups.push(group);
    }

    fn maybe_push_cat(&mut self) {
        if self.group.ast_count - self.group.alt_count - self.group.cat_count == 2 {
            self.group.cat_count += 1;
        }
    }

    fn pop_group(&mut self) {
        self.maybe_push_cat();
        self.pop_alts();
        self.group.ast_count += 1;
    }

    fn pop_alts(&mut self) {
        self.pop_cats();
        if self.group.alt_count == 0 {
            return;
        }
        let asts = self
            .asts
            .split_off(self.asts.len() - (self.group.alt_count + 1));
        self.asts.push(Ast::Alt(asts));
        self.group.ast_count -= self.group.alt_count;
        self.group.alt_count = 0;
    }

    fn pop_cats(&mut self) {
        if self.group.cat_count == 0 {
            return;
        }
        let asts = self
            .asts
            .split_off(self.asts.len() - (self.group.cat_count + 1));
        self.asts.push(Ast::Cat(asts));
        self.group.ast_count -= self.group.cat_count;
        self.group.cat_count = 0;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Group {
    ast_count: usize,
    alt_count: usize,
    cat_count: usize,
}
