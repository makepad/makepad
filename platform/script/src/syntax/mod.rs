//! A lossless syntax tree for whole Splash files, and structural edits on it.
//!
//! The tree is built over [`crate::tokenizer::lex`], the tokenizer scripts run
//! on. Every node carries the byte range it was written in; nothing is
//! evaluated and nothing is normalised. Comments and whitespace are the gaps
//! between node ranges, so [`Syntax::print`] (the tree's nodes with the source
//! text between them) is byte-for-byte the source it was parsed from.
//!
//! * [`parse`] reads a file: statements (`let`, `fn`, `use`, `if` / `for` /
//!   `while` / `loop` / `match` / `try` blocks, assignments, expression
//!   statements), calls, objects, arrays and literals.
//! * Queries find nodes by callee path ([`Syntax::find_calls`]), `let` name
//!   ([`Syntax::find_let`]), `id:` field ([`Syntax::find_by_id`]) or path
//!   ([`Syntax::at_path`]) and answer with a [`Loc`], an address into the tree.
//! * Edits ([`edit`]) replace only the bytes that change, parse the result again
//!   and check that the new tree has exactly the intended shape. The text they
//!   insert is written by [`write`] from a Rust [`write::Value`]; that is the
//!   one way text is produced.
//!
//! Operator sequences are kept as written, left to right ([`Kind::Chain`]);
//! precedence is the VM's, not modelled here. Where the VM decides by
//! evaluation (a `{` after a name is an object only when it follows
//! the name closely or the name is a type) the tree follows the same lexical
//! rules the VM's parser uses.

mod parse;
pub mod edit;
pub mod write;

use std::ops::Range;

pub use edit::{At, EditError};
pub use parse::parse_value;

/// A parse failure: where, and what was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub at: usize,
    pub message: String,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (byte {})", self.message, self.at)
    }
}

/// One node as written.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub span: Range<usize>,
    pub kind: Kind,
}

/// A `{ ... }` of statements, or a file (no braces).
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Byte offsets of `{` and `}`; `None` for a whole file.
    pub open: Option<usize>,
    pub close: Option<usize>,
    pub stmts: Vec<Node>,
}

/// One entry of an object: `name: value`, `name := value`, `name +: value`,
/// or a bare entry (a value or statement, `name` is `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: Option<String>,
    pub name_span: Range<usize>,
    /// `:`, `:=`, `+:`, ... ; empty for a bare entry.
    pub op: String,
    /// The whole entry, from its name to the end of its value.
    pub span: Range<usize>,
    pub value: Node,
}

/// `+ x` in an operator sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub op: String,
    pub op_span: Range<usize>,
    pub operand: Node,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    /// The parameter with its type annotation.
    pub span: Range<usize>,
}

/// `pattern => body` in a `match`.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub pattern: Node,
    pub body: Node,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Number(f64),
    /// A string, escapes resolved.
    Str(String),
    /// `@name` (an id: an ease, a mode, an input name).
    Name(String),
    /// `#rrggbb` / `#rgb`, as written without the `#`.
    Color(String),
    /// A bare identifier or dotted path (`true`, `accent`, `mod.motion.Rect`,
    /// `mod.std.*`).
    Ident(String),
    /// `callee(args...)`. A plain path callee is `Ident` (`game.model`).
    Call { callee: Box<Node>, args: Vec<Node>, open: usize, close: usize },
    /// `Type{...}` or a bare `{...}`.
    Object { ty: Option<String>, fields: Vec<Field>, open: usize, close: usize },
    /// `[a, b, ...]`.
    Array { items: Vec<Node>, open: usize, close: usize },
    /// `(a)` / `(a, b)`.
    Paren { items: Vec<Node>, open: usize, close: usize },
    /// `base[index]`.
    Index { base: Box<Node>, index: Box<Node>, open: usize, close: usize },
    /// `base.name` after something that is not a plain path (`f(x).y`, `a[i].b`).
    Member { base: Box<Node>, name: String, name_span: Range<usize> },
    /// `name: value` / `a.b := value` as a statement: a file or block that is the
    /// body of an object.
    Entry { name: String, name_span: Range<usize>, op: String, value: Box<Node> },
    /// `-x`, `!x`, `..x`, `.name` (a field of `me`).
    Unary { op: String, operand: Box<Node> },
    /// `first op operand op operand ...`, in source order.
    Chain { first: Box<Node>, rest: Vec<Link> },
    /// `target op value` with `op` one of `=`, `+=`, `:=` ...
    Assign { target: Box<Node>, op: String, op_span: Range<usize>, value: Box<Node> },
    /// `let a = v`, `var a = v`, `let [a, b] = v`, `let {a, b} = v`.
    Let { keyword: String, names: Vec<String>, pattern_span: Range<usize>, value: Option<Box<Node>> },
    /// `fn name(params) { body }`, `fn(params) { body }`, `|a, b| body`.
    Fn { name: Option<String>, name_span: Range<usize>, params: Vec<Param>, ret: Option<Range<usize>>, body: Box<Node> },
    /// `use path`.
    Use { path: Box<Node> },
    /// A braced statement list.
    Block(Block),
    If { cond: Box<Node>, then: Box<Node>, otherwise: Option<Box<Node>> },
    For { binding: Range<usize>, iter: Box<Node>, body: Box<Node> },
    While { cond: Box<Node>, body: Box<Node> },
    Loop { body: Box<Node> },
    Match { subject: Box<Node>, arms: Vec<Arm>, open: usize, close: usize },
    /// `try body catch handler`.
    Try { body: Box<Node>, handler: Option<Box<Node>> },
    /// `ok body`.
    Ok { body: Box<Node> },
    /// `return v`, `break v`, `continue`.
    Jump { keyword: String, value: Option<Box<Node>> },
}

/// An address of a node: child indexes from the root (see [`Node::children`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Loc(pub Vec<usize>);

impl Loc {
    pub fn child(&self, index: usize) -> Loc {
        let mut path = self.0.clone();
        path.push(index);
        Loc(path)
    }
    pub fn parent(&self) -> Option<Loc> {
        let mut path = self.0.clone();
        path.pop()?;
        Some(Loc(path))
    }
}

/// One step of a path into a document: an object field, an element index, or
/// the element whose `id:` field is the string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Field(String),
    Index(usize),
    Id(String),
}

impl Step {
    /// `layers.2.fill` / `layers.#title.fill`: fields, indexes, `#id`s.
    pub fn parse_path(path: &str) -> Vec<Step> {
        path.split('.')
            .filter(|p| !p.is_empty())
            .map(|p| match p.parse::<usize>() {
                Ok(i) => Step::Index(i),
                Err(_) => match p.strip_prefix('#') {
                    Some(id) => Step::Id(id.to_string()),
                    None => Step::Field(p.to_string()),
                },
            })
            .collect()
    }
}

impl Node {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.clone()]
    }

    pub fn number(&self) -> Option<f64> {
        match self.kind {
            Kind::Number(value) => Some(value),
            _ => None,
        }
    }
    pub fn string(&self) -> Option<&str> {
        match &self.kind {
            Kind::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            Kind::Name(name) => Some(name),
            _ => None,
        }
    }
    pub fn ident(&self) -> Option<&str> {
        match &self.kind {
            Kind::Ident(name) => Some(name),
            _ => None,
        }
    }

    /// The path of a call's callee when it is a plain dotted path (`game.model`).
    pub fn callee_path(&self) -> Option<&str> {
        match &self.kind {
            Kind::Call { callee, .. } => callee.ident(),
            _ => None,
        }
    }

    /// A call whose callee is a plain path: the path and the arguments.
    pub fn as_call(&self) -> Option<(&str, &[Node])> {
        match &self.kind {
            Kind::Call { callee, args, .. } => Some((callee.ident()?, args)),
            _ => None,
        }
    }

    /// Anything but a literal, a name, a call of a path, an object or an
    /// array: operators, groups, indexing, members of calls, functions,
    /// statements.
    pub fn is_expression(&self) -> bool {
        match &self.kind {
            Kind::Number(_) | Kind::Str(_) | Kind::Name(_) | Kind::Color(_) | Kind::Ident(_) | Kind::Object { .. } | Kind::Array { .. } => false,
            Kind::Call { .. } => self.as_call().is_none(),
            _ => true,
        }
    }

    /// The arguments of a call to the path `which`.
    pub fn call(&self, which: &str) -> Option<&[Node]> {
        match &self.kind {
            Kind::Call { callee, args, .. } if callee.ident() == Some(which) => Some(args),
            _ => None,
        }
    }

    pub fn object(&self) -> Option<(Option<&str>, &[Field])> {
        match &self.kind {
            Kind::Object { ty, fields, .. } => Some((ty.as_deref(), fields)),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Node]> {
        match &self.kind {
            Kind::Array { items, .. } => Some(items),
            _ => None,
        }
    }

    pub fn block(&self) -> Option<&Block> {
        match &self.kind {
            Kind::Block(block) => Some(block),
            _ => None,
        }
    }

    /// The field `name` of an object value.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.object()?.1.iter().find(|field| field.name.as_deref() == Some(name))
    }

    /// The `let` name this node binds (a simple `let a = ...`).
    pub fn let_name(&self) -> Option<&str> {
        match &self.kind {
            Kind::Let { names, .. } if names.len() == 1 => Some(&names[0]),
            _ => None,
        }
    }

    /// The `id:` of an object: the string (or bare identifier) its `id` field holds.
    pub fn id(&self) -> Option<&str> {
        let field = self.field("id")?;
        match &field.value.kind {
            Kind::Str(s) => Some(s),
            Kind::Ident(s) => Some(s),
            _ => None,
        }
    }

    /// The nodes inside this one, in source order. A [`Loc`] step indexes this list.
    pub fn children(&self) -> Vec<&Node> {
        let mut out: Vec<&Node> = Vec::new();
        match &self.kind {
            Kind::Number(_) | Kind::Str(_) | Kind::Name(_) | Kind::Color(_) | Kind::Ident(_) => {}
            Kind::Call { callee, args, .. } => {
                out.push(callee);
                out.extend(args.iter());
            }
            Kind::Object { fields, .. } => out.extend(fields.iter().map(|f| &f.value)),
            Kind::Array { items, .. } | Kind::Paren { items, .. } => out.extend(items.iter()),
            Kind::Index { base, index, .. } => {
                out.push(base);
                out.push(index);
            }
            Kind::Member { base, .. } => out.push(base),
            Kind::Entry { value, .. } => out.push(value),
            Kind::Unary { operand, .. } => out.push(operand),
            Kind::Chain { first, rest } => {
                out.push(first);
                out.extend(rest.iter().map(|l| &l.operand));
            }
            Kind::Assign { target, value, .. } => {
                out.push(target);
                out.push(value);
            }
            Kind::Let { value, .. } => out.extend(value.iter().map(|v| &**v)),
            Kind::Fn { body, .. } => out.push(body),
            Kind::Use { path } => out.push(path),
            Kind::Block(block) => out.extend(block.stmts.iter()),
            Kind::If { cond, then, otherwise } => {
                out.push(cond);
                out.push(then);
                out.extend(otherwise.iter().map(|o| &**o));
            }
            Kind::For { iter, body, .. } => {
                out.push(iter);
                out.push(body);
            }
            Kind::While { cond, body } => {
                out.push(cond);
                out.push(body);
            }
            Kind::Loop { body } | Kind::Ok { body } => out.push(body),
            Kind::Match { subject, arms, .. } => {
                out.push(subject);
                for arm in arms {
                    out.push(&arm.pattern);
                    out.push(&arm.body);
                }
            }
            Kind::Try { body, handler } => {
                out.push(body);
                out.extend(handler.iter().map(|h| &**h));
            }
            Kind::Jump { value, .. } => out.extend(value.iter().map(|v| &**v)),
        }
        out
    }

    /// The node at `loc` below this one.
    pub fn at(&self, loc: &[usize]) -> Option<&Node> {
        let mut node = self;
        for &step in loc {
            node = node.children().get(step).copied()?;
        }
        Some(node)
    }

    /// Visit this node and everything inside it, depth first, in source order.
    pub fn walk<'a>(&'a self, loc: &Loc, f: &mut dyn FnMut(&Loc, &'a Node)) {
        f(loc, self);
        for (i, child) in self.children().into_iter().enumerate() {
            child.walk(&loc.child(i), f);
        }
    }

    /// The value one step below this one.
    fn step(&self, step: &Step) -> Option<(usize, &Node)> {
        match step {
            Step::Field(name) => {
                let (_, fields) = self.object()?;
                let i = fields.iter().position(|f| f.name.as_deref() == Some(name))?;
                Some((i, &fields[i].value))
            }
            Step::Index(i) => match &self.kind {
                Kind::Array { items, .. } | Kind::Paren { items, .. } => items.get(*i).map(|n| (*i, n)),
                Kind::Call { args, .. } => args.get(*i).map(|n| (*i + 1, n)),
                Kind::Block(block) => block.stmts.get(*i).map(|n| (*i, n)),
                _ => None,
            },
            Step::Id(id) => {
                let elements: Vec<(usize, &Node)> = match &self.kind {
                    Kind::Array { items, .. } | Kind::Paren { items, .. } => items.iter().enumerate().collect(),
                    Kind::Call { args, .. } => args.iter().enumerate().map(|(i, n)| (i + 1, n)).collect(),
                    Kind::Block(block) => block.stmts.iter().enumerate().collect(),
                    Kind::Object { fields, .. } => fields.iter().map(|f| &f.value).enumerate().collect(),
                    _ => return None,
                };
                elements.into_iter().find(|(_, n)| n.id() == Some(id))
            }
        }
    }
}

/// A parsed file: its text and its tree. The text is the tree's source of truth;
/// every edit returns a new `Syntax`.
#[derive(Clone, Debug, PartialEq)]
pub struct Syntax {
    source: String,
    root: Node,
}

/// Parse a whole file.
pub fn parse(source: &str) -> Result<Syntax, SyntaxError> {
    let root = parse::parse_file(source)?;
    Ok(Syntax { source: source.to_string(), root })
}

impl Syntax {
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The file as a block of statements.
    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn statements(&self) -> &[Node] {
        &self.root.block().expect("the root is a block").stmts
    }

    /// The tree written back out: each node's text with the source gaps (comments,
    /// whitespace, separators) between them. Equal to [`source`](Self::source).
    pub fn print(&self) -> String {
        let mut out = String::with_capacity(self.source.len());
        print_node(&self.source, &self.root, 0..self.source.len(), &mut out);
        out
    }

    pub fn text(&self, node: &Node) -> &str {
        node.text(&self.source)
    }

    pub fn node(&self, loc: &Loc) -> Option<&Node> {
        self.root.at(&loc.0)
    }

    /// Visit every node, depth first, in source order.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&Loc, &'a Node)) {
        self.root.walk(&Loc::default(), f);
    }

    /// Every call whose callee is the path `callee` (`game.model`), anywhere in
    /// the file, in source order.
    pub fn find_calls(&self, callee: &str) -> Vec<Loc> {
        let mut out = Vec::new();
        self.walk(&mut |loc, node| {
            if node.callee_path() == Some(callee) {
                out.push(loc.clone());
            }
        });
        out
    }

    /// Every `let name = ...` / `var name = ...` / `fn name(...)` binding `name`.
    pub fn find_lets(&self, name: &str) -> Vec<Loc> {
        let mut out = Vec::new();
        self.walk(&mut |loc, node| match &node.kind {
            Kind::Let { names, .. } if names.iter().any(|n| n == name) => out.push(loc.clone()),
            Kind::Fn { name: Some(n), .. } if n == name => out.push(loc.clone()),
            _ => {}
        });
        out
    }

    /// The first binding of `name` (see [`find_lets`](Self::find_lets)).
    pub fn find_let(&self, name: &str) -> Option<Loc> {
        self.find_lets(name).into_iter().next()
    }

    /// Every object whose `id:` field is `id` (an element of a data list, a
    /// call argument, a field value).
    pub fn find_by_id(&self, id: &str) -> Vec<Loc> {
        let mut out = Vec::new();
        self.walk(&mut |loc, node| {
            if node.id() == Some(id) {
                out.push(loc.clone());
            }
        });
        out
    }

    /// The node `steps` below `from`.
    pub fn at_path(&self, from: &Loc, steps: &[Step]) -> Option<Loc> {
        let mut loc = from.clone();
        let mut node = self.node(from)?;
        for step in steps {
            let (index, next) = node.step(step)?;
            loc = loc.child(index);
            node = next;
        }
        Some(loc)
    }

    /// 1-based line and column of byte `at`.
    pub fn line_col(&self, at: usize) -> (usize, usize) {
        line_col(&self.source, at)
    }
}

fn print_node(source: &str, node: &Node, span: Range<usize>, out: &mut String) {
    // The node's own bytes are its children's bytes with the gaps between.
    let mut at = span.start;
    let put = |out: &mut String, upto: usize, at: &mut usize| {
        out.push_str(&source[*at..upto]);
        *at = upto;
    };
    // Object fields and match arms own bytes the generic child list does not
    // show (names, operators), which fall in the gaps; they are source text.
    for child in node.children() {
        put(out, child.span.start, &mut at);
        print_node(source, child, child.span.clone(), out);
        at = child.span.end;
    }
    put(out, span.end, &mut at);
}

/// 1-based line and column of byte `at` in `source`.
pub fn line_col(source: &str, at: usize) -> (usize, usize) {
    let at = at.min(source.len());
    let before = &source[..at];
    let line = before.matches('\n').count() + 1;
    let col = before.rfind('\n').map_or(before.chars().count(), |nl| before[nl + 1..].chars().count()) + 1;
    (line, col)
}

/// The string the whole tree covers, as one check that nodes nest in order
/// without overlap and stay inside their parent. Returns the first violation.
pub fn check_nesting(syntax: &Syntax) -> Result<(), String> {
    fn visit(node: &Node, parent: &Range<usize>, source: &str) -> Result<(), String> {
        if node.span.start < parent.start || node.span.end > parent.end || node.span.start > node.span.end {
            return Err(format!("node {:?} at {:?} is outside its parent {:?}", node.kind_name(), node.span, parent));
        }
        let mut at = node.span.start;
        for child in node.children() {
            if child.span.start < at {
                return Err(format!("{} at {:?} overlaps the node before it (ends at {at}) in {:?}", child.kind_name(), child.span, source.get(node.span.clone()).unwrap_or("")));
            }
            visit(child, &node.span, source)?;
            at = child.span.end;
        }
        Ok(())
    }
    visit(&syntax.root, &(0..syntax.source.len()), &syntax.source)
}

impl Node {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::Number(_) => "number",
            Kind::Str(_) => "string",
            Kind::Name(_) => "name",
            Kind::Color(_) => "color",
            Kind::Ident(_) => "identifier",
            Kind::Call { .. } => "call",
            Kind::Object { .. } => "object",
            Kind::Array { .. } => "array",
            Kind::Paren { .. } => "group",
            Kind::Index { .. } => "index",
            Kind::Member { .. } => "member",
            Kind::Unary { .. } => "unary",
            Kind::Entry { .. } => "entry",
            Kind::Chain { .. } => "operators",
            Kind::Assign { .. } => "assignment",
            Kind::Let { .. } => "let",
            Kind::Fn { .. } => "fn",
            Kind::Use { .. } => "use",
            Kind::Block(_) => "block",
            Kind::If { .. } => "if",
            Kind::For { .. } => "for",
            Kind::While { .. } => "while",
            Kind::Loop { .. } => "loop",
            Kind::Match { .. } => "match",
            Kind::Try { .. } => "try",
            Kind::Ok { .. } => "ok",
            Kind::Jump { .. } => "jump",
        }
    }
}

#[cfg(test)]
mod tests;
