/// LaTeX math parser. Converts a LaTeX math string into a `Vec<MathNode>` AST.
/// No external dependencies — just pure Rust string parsing.

#[derive(Debug, Clone, PartialEq)]
pub enum MathNode {
    /// A single character glyph (letter, digit, operator symbol)
    Char(char),
    /// A command-produced symbol like \alpha, \sum, \int
    Symbol(Symbol),
    /// Fraction: \frac{numerator}{denominator}
    Frac(Vec<MathNode>, Vec<MathNode>),
    /// Square root or nth root: \sqrt{content} or \sqrt[n]{content}
    Sqrt(Option<Vec<MathNode>>, Vec<MathNode>),
    /// Superscript: base^{exponent} (base is previous node, stored separately)
    Superscript(Vec<MathNode>),
    /// Subscript: base_{index}
    Subscript(Vec<MathNode>),
    /// Both super and subscript on previous node
    SubSuperscript(Vec<MathNode>, Vec<MathNode>),
    /// Accent above: \hat{x}, \bar{x}, \tilde{x}, etc.
    Accent(AccentKind, Vec<MathNode>),
    /// Grouped content: {a + b}
    Group(Vec<MathNode>),
    /// Left-right delimited: \left( ... \right)
    LeftRight(Delimiter, Vec<MathNode>, Delimiter),
    /// Matrix/array environment: \begin{pmatrix} ... \end{pmatrix}
    Matrix(MatrixKind, Vec<Vec<Vec<MathNode>>>), // rows of cells
    /// Whitespace spacing commands: \, \; \quad etc.
    Space(SpaceWidth),
    /// \text{...} — pass-through text
    Text(String),
    /// Operator name like \sin, \cos, \lim (upright text)
    OperatorName(String),
    /// \overline{...}, \underline{...}
    Overline(Vec<MathNode>),
    Underline(Vec<MathNode>),
    /// \overbrace{...} / \underbrace{...}
    Overbrace(Vec<MathNode>),
    Underbrace(Vec<MathNode>),
    /// \mathbf{...}, \mathit{...}, etc.
    MathVariant(MathVariant, Vec<MathNode>),
    /// \sum, \prod, \int with limits — encoded as the symbol followed by Sub/Superscript
    /// Big operators are just Symbol nodes; limits are handled by sub/superscript attachment.

    /// \color{red}{content} or \textcolor{red}{content}
    Color(String, Vec<MathNode>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    // Greek lowercase
    Alpha,
    Beta,
    Gamma,
    Delta,
    Epsilon,
    Zeta,
    Eta,
    Theta,
    Iota,
    Kappa,
    Lambda,
    Mu,
    Nu,
    Xi,
    Omicron,
    Pi,
    Rho,
    Sigma,
    Tau,
    Upsilon,
    Phi,
    Chi,
    Psi,
    Omega,
    VarEpsilon,
    VarTheta,
    VarPhi,
    VarPi,
    VarRho,
    VarSigma,
    // Greek uppercase
    UpperGamma,
    UpperDelta,
    UpperTheta,
    UpperLambda,
    UpperXi,
    UpperPi,
    UpperSigma,
    UpperUpsilon,
    UpperPhi,
    UpperPsi,
    UpperOmega,
    // Big operators
    Sum,
    Prod,
    Coprod,
    Int,
    Iint,
    Iiint,
    Oint,
    Bigcup,
    Bigcap,
    Bigsqcup,
    Bigvee,
    Bigwedge,
    Bigoplus,
    Bigotimes,
    Bigodot,
    // Binary operators
    Plus,
    Minus,
    Times,
    Div,
    Cdot,
    Star,
    Circ,
    Bullet,
    Diamond,
    Pm,
    Mp,
    Ast,
    Dagger,
    Ddagger,
    Setminus,
    Wr,
    // Relations
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    Leq,
    Geq,
    Ll,
    Gg,
    Prec,
    Succ,
    Preceq,
    Succeq,
    Sim,
    Simeq,
    Approx,
    Cong,
    Equiv,
    Subset,
    Supset,
    Subseteq,
    Supseteq,
    In,
    Ni,
    Notin,
    Propto,
    Parallel,
    Perp,
    Mid,
    Vdash,
    Dashv,
    Models,
    // Arrows
    LeftArrow,
    RightArrow,
    LeftRightArrow,
    Uparrow,
    Downarrow,
    DoubleLeftArrow,
    DoubleRightArrow,
    DoubleLeftRightArrow,
    Mapsto,
    LongRightArrow,
    LongLeftArrow,
    LongLeftRightArrow,
    Hookrightarrow,
    Hookleftarrow,
    Nearrow,
    Searrow,
    Nwarrow,
    Swarrow,
    // Dots
    Ldots,
    Cdots,
    Vdots,
    Ddots,
    // Misc
    Infty,
    Partial,
    Nabla,
    Forall,
    Exists,
    Nexists,
    Emptyset,
    Aleph,
    Hbar,
    Ell,
    Wp,
    Re,
    Im,
    Angle,
    Triangle,
    Triangledown,
    Neg,
    Flat,
    Natural,
    Sharp,
    Clubsuit,
    Diamondsuit,
    Heartsuit,
    Spadesuit,
    // Delimiters (when used as symbols, not \left/\right)
    Langle,
    Rangle,
    Lfloor,
    Rfloor,
    Lceil,
    Rceil,
    Vert,
    DoubleVert,
    // Punctuation
    Comma,
    Semicolon,
    Colon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    Paren,      // ( )
    Bracket,    // [ ]
    Brace,      // \{ \}
    Vert,       // | or \vert
    DoubleVert, // \| or \Vert
    Angle,      // \langle \rangle
    Floor,      // \lfloor \rfloor
    Ceil,       // \lceil \rceil
    None,       // . (invisible delimiter)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccentKind {
    Hat,
    Bar,
    Tilde,
    Vec,
    Dot,
    Ddot,
    Acute,
    Grave,
    Check,
    Breve,
    WideHat,
    WideTilde,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixKind {
    Plain,      // matrix
    Paren,      // pmatrix
    Bracket,    // bmatrix
    Brace,      // Bmatrix
    Vert,       // vmatrix
    DoubleVert, // Vmatrix
    Cases,      // cases
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceWidth {
    Thin,    // \,
    Medium,  // \:
    Thick,   // \;
    Quad,    // \quad
    QQuad,   // \qquad
    NegThin, // \!
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathVariant {
    Bold,           // \mathbf
    Italic,         // \mathit
    Roman,          // \mathrm
    SansSerif,      // \mathsf
    Monospace,      // \mathtt
    Calligraphic,   // \mathcal
    Fraktur,        // \mathfrak
    BlackboardBold, // \mathbb
    BoldItalic,     // \boldsymbol
}

// --- Parser implementation ---

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn eat_command_name(&mut self) -> &'a str {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() {
                self.advance();
            } else {
                break;
            }
        }
        &self.input[start..self.pos]
    }

    fn _parse_group(&mut self) -> Vec<MathNode> {
        // Expects '{' already consumed, reads until matching '}'
        let mut depth = 1u32;
        let mut nodes = Vec::new();
        while depth > 0 {
            match self.peek() {
                None => break,
                Some('{') => {
                    self.advance();
                    depth += 1;
                    // Parse inner group
                    let inner = self.parse_until(|p| p.peek() == Some('}'));
                    if self.peek() == Some('}') {
                        self.advance();
                        depth -= 1;
                    }
                    nodes.push(MathNode::Group(inner));
                    if depth == 0 {
                        return nodes; // oops, we went one too far
                    }
                }
                _ => {
                    // Use parse_until to handle the content properly
                    let inner = self.parse_until(|p| matches!(p.peek(), Some('}') | None));
                    nodes.extend(inner);
                    if self.peek() == Some('}') {
                        self.advance();
                        depth -= 1;
                    }
                }
            }
        }
        nodes
    }

    fn parse_brace_group(&mut self) -> Vec<MathNode> {
        self.skip_whitespace();
        if self.peek() == Some('{') {
            self.advance();
            let content = self.parse_until(|p| p.peek() == Some('}'));
            if self.peek() == Some('}') {
                self.advance();
            }
            content
        } else {
            // Single token as argument
            if let Some(node) = self.parse_single() {
                vec![node]
            } else {
                vec![]
            }
        }
    }

    fn parse_bracket_group(&mut self) -> Option<Vec<MathNode>> {
        self.skip_whitespace();
        if self.peek() == Some('[') {
            self.advance();
            let content = self.parse_until(|p| p.peek() == Some(']'));
            if self.peek() == Some(']') {
                self.advance();
            }
            Some(content)
        } else {
            None
        }
    }

    /// Parse a single token/node (for unbraced arguments like x^2)
    fn parse_single(&mut self) -> Option<MathNode> {
        self.skip_whitespace();
        match self.peek()? {
            '\\' => {
                self.advance();
                match self.peek() {
                    Some(c) if c.is_ascii_alphabetic() => {
                        let name = self.eat_command_name();
                        self.command_to_node(name)
                    }
                    Some(c) => {
                        self.advance();
                        match c {
                            '{' => Some(MathNode::Symbol(Symbol::Langle)), // \{
                            '}' => Some(MathNode::Symbol(Symbol::Rangle)), // \}
                            ',' => Some(MathNode::Space(SpaceWidth::Thin)),
                            ';' => Some(MathNode::Space(SpaceWidth::Thick)),
                            ':' => Some(MathNode::Space(SpaceWidth::Medium)),
                            '!' => Some(MathNode::Space(SpaceWidth::NegThin)),
                            '|' => Some(MathNode::Symbol(Symbol::DoubleVert)),
                            ' ' => Some(MathNode::Space(SpaceWidth::Medium)),
                            _ => Some(MathNode::Char(c)),
                        }
                    }
                    None => None,
                }
            }
            '{' => {
                self.advance();
                let content = self.parse_until(|p| p.peek() == Some('}'));
                if self.peek() == Some('}') {
                    self.advance();
                }
                Some(MathNode::Group(content))
            }
            c if is_math_char(c) => {
                self.advance();
                Some(MathNode::Char(c))
            }
            _ => {
                self.advance();
                None
            }
        }
    }

    fn parse_until<F>(&mut self, stop: F) -> Vec<MathNode>
    where
        F: Fn(&Parser) -> bool,
    {
        let mut nodes = Vec::new();
        loop {
            self.skip_whitespace();
            if stop(self) || self.peek().is_none() {
                break;
            }
            if let Some(node) = self.parse_one() {
                // Check for sub/superscript attachment
                let node = self.maybe_attach_scripts(node);
                nodes.push(node);
            }
        }
        nodes
    }

    fn maybe_attach_scripts(&mut self, base: MathNode) -> MathNode {
        self.skip_whitespace();
        match self.peek() {
            Some('^') => {
                self.advance();
                let sup = self.parse_brace_group();
                self.skip_whitespace();
                if self.peek() == Some('_') {
                    self.advance();
                    let sub = self.parse_brace_group();
                    // Return base, then SubSuperscript
                    // We need to wrap: base becomes a group that includes the attachment
                    // Actually, the standard approach: return a sequence
                    // For simplicity, we push base into nodes outside, and return SubSuperscript
                    // But we're called with base already. Let's use Group to bundle them.
                    return MathNode::Group(vec![base, MathNode::SubSuperscript(sub, sup)]);
                }
                MathNode::Group(vec![base, MathNode::Superscript(sup)])
            }
            Some('_') => {
                self.advance();
                let sub = self.parse_brace_group();
                self.skip_whitespace();
                if self.peek() == Some('^') {
                    self.advance();
                    let sup = self.parse_brace_group();
                    return MathNode::Group(vec![base, MathNode::SubSuperscript(sub, sup)]);
                }
                MathNode::Group(vec![base, MathNode::Subscript(sub)])
            }
            _ => base,
        }
    }

    fn parse_one(&mut self) -> Option<MathNode> {
        self.skip_whitespace();
        let c = self.peek()?;
        match c {
            '\\' => {
                self.advance();
                match self.peek() {
                    Some(c) if c.is_ascii_alphabetic() => {
                        let name = self.eat_command_name();
                        self.command_to_node(name)
                    }
                    Some(c) => {
                        self.advance();
                        match c {
                            '{' => Some(MathNode::Char('{')),
                            '}' => Some(MathNode::Char('}')),
                            '\\' => Some(MathNode::Char('\n')), // line break in matrix
                            ',' => Some(MathNode::Space(SpaceWidth::Thin)),
                            ';' => Some(MathNode::Space(SpaceWidth::Thick)),
                            ':' => Some(MathNode::Space(SpaceWidth::Medium)),
                            '!' => Some(MathNode::Space(SpaceWidth::NegThin)),
                            '|' => Some(MathNode::Symbol(Symbol::DoubleVert)),
                            ' ' => Some(MathNode::Space(SpaceWidth::Medium)),
                            _ => Some(MathNode::Char(c)),
                        }
                    }
                    None => None,
                }
            }
            '{' => {
                self.advance();
                let content = self.parse_until(|p| p.peek() == Some('}'));
                if self.peek() == Some('}') {
                    self.advance();
                }
                Some(MathNode::Group(content))
            }
            // These are handled by caller contexts
            '}' | ']' => None,
            '&' => {
                self.advance();
                Some(MathNode::Char('&')) // Column separator in matrices
            }
            '\'' => {
                self.advance();
                Some(MathNode::Symbol(Symbol::DoubleVert)) // prime — will fix in layout
            }
            _ if is_math_char(c) => {
                self.advance();
                Some(MathNode::Char(c))
            }
            _ => {
                self.advance();
                None
            }
        }
    }

    fn command_to_node(&mut self, name: &str) -> Option<MathNode> {
        // Commands that take arguments
        match name {
            "frac" | "dfrac" | "tfrac" => {
                let num = self.parse_brace_group();
                let den = self.parse_brace_group();
                return Some(MathNode::Frac(num, den));
            }
            "sqrt" => {
                let index = self.parse_bracket_group();
                let content = self.parse_brace_group();
                return Some(MathNode::Sqrt(index, content));
            }
            "hat" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Hat, c));
            }
            "bar" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Bar, c));
            }
            "tilde" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Tilde, c));
            }
            "vec" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Vec, c));
            }
            "dot" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Dot, c));
            }
            "ddot" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Ddot, c));
            }
            "acute" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Acute, c));
            }
            "grave" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Grave, c));
            }
            "check" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Check, c));
            }
            "breve" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::Breve, c));
            }
            "widehat" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::WideHat, c));
            }
            "widetilde" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Accent(AccentKind::WideTilde, c));
            }
            "overline" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Overline(c));
            }
            "underline" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Underline(c));
            }
            "overbrace" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Overbrace(c));
            }
            "underbrace" => {
                let c = self.parse_brace_group();
                return Some(MathNode::Underbrace(c));
            }
            "mathbf" | "bf" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::Bold, c));
            }
            "mathit" | "it" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::Italic, c));
            }
            "mathrm" | "rm" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::Roman, c));
            }
            "mathsf" | "sf" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::SansSerif, c));
            }
            "mathtt" | "tt" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::Monospace, c));
            }
            "mathcal" | "cal" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::Calligraphic, c));
            }
            "mathfrak" | "frak" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::Fraktur, c));
            }
            "mathbb" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::BlackboardBold, c));
            }
            "boldsymbol" => {
                let c = self.parse_brace_group();
                return Some(MathNode::MathVariant(MathVariant::BoldItalic, c));
            }
            "text" | "textrm" | "textit" | "textbf" => {
                let t = self.parse_brace_text();
                return Some(MathNode::Text(t));
            }
            "operatorname" => {
                let t = self.parse_brace_text();
                return Some(MathNode::OperatorName(t));
            }
            "color" | "textcolor" => {
                let color = self.parse_brace_text();
                let content = self.parse_brace_group();
                return Some(MathNode::Color(color, content));
            }
            "left" => {
                return self.parse_left_right();
            }
            "begin" => {
                let env_name = self.parse_brace_text();
                return self.parse_environment(&env_name);
            }
            "quad" => return Some(MathNode::Space(SpaceWidth::Quad)),
            "qquad" => return Some(MathNode::Space(SpaceWidth::QQuad)),
            _ => {}
        }

        // Operator names (rendered upright)
        if let Some(op) = operator_name(name) {
            return Some(MathNode::OperatorName(op.to_string()));
        }

        // Symbol lookup
        if let Some(sym) = symbol_from_name(name) {
            return Some(MathNode::Symbol(sym));
        }

        // Unknown command — try to render as text
        Some(MathNode::Text(format!("\\{}", name)))
    }

    fn parse_brace_text(&mut self) -> String {
        self.skip_whitespace();
        if self.peek() == Some('{') {
            self.advance();
            let start = self.pos;
            let mut depth = 1u32;
            while let Some(c) = self.peek() {
                match c {
                    '{' => {
                        depth += 1;
                        self.advance();
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let text = self.input[start..self.pos].to_string();
                            self.advance(); // consume '}'
                            return text;
                        }
                        self.advance();
                    }
                    _ => {
                        self.advance();
                    }
                }
            }
            self.input[start..self.pos].to_string()
        } else {
            // single char
            if let Some(c) = self.advance() {
                c.to_string()
            } else {
                String::new()
            }
        }
    }

    fn parse_left_right(&mut self) -> Option<MathNode> {
        self.skip_whitespace();
        let left_delim = self.parse_delimiter()?;
        let content = self.parse_until(|p| p.remaining().starts_with("\\right"));
        // consume \right
        if self.remaining().starts_with("\\right") {
            self.pos += 6; // skip \right
        }
        self.skip_whitespace();
        let right_delim = self.parse_delimiter().unwrap_or(Delimiter::None);
        Some(MathNode::LeftRight(left_delim, content, right_delim))
    }

    fn parse_delimiter(&mut self) -> Option<Delimiter> {
        self.skip_whitespace();
        match self.peek()? {
            '(' => {
                self.advance();
                Some(Delimiter::Paren)
            }
            ')' => {
                self.advance();
                Some(Delimiter::Paren)
            }
            '[' => {
                self.advance();
                Some(Delimiter::Bracket)
            }
            ']' => {
                self.advance();
                Some(Delimiter::Bracket)
            }
            '|' => {
                self.advance();
                Some(Delimiter::Vert)
            }
            '.' => {
                self.advance();
                Some(Delimiter::None)
            }
            '\\' => {
                self.advance();
                match self.peek() {
                    Some('{') | Some('}') => {
                        self.advance();
                        Some(Delimiter::Brace)
                    }
                    Some('|') => {
                        self.advance();
                        Some(Delimiter::DoubleVert)
                    }
                    Some(c) if c.is_ascii_alphabetic() => {
                        let name = self.eat_command_name();
                        match name {
                            "langle" | "rangle" => Some(Delimiter::Angle),
                            "lfloor" | "rfloor" => Some(Delimiter::Floor),
                            "lceil" | "rceil" => Some(Delimiter::Ceil),
                            "vert" => Some(Delimiter::Vert),
                            "Vert" => Some(Delimiter::DoubleVert),
                            "lbrace" | "rbrace" => Some(Delimiter::Brace),
                            _ => Some(Delimiter::None),
                        }
                    }
                    _ => Some(Delimiter::None),
                }
            }
            _ => None,
        }
    }

    fn parse_environment(&mut self, name: &str) -> Option<MathNode> {
        let kind = match name {
            "matrix" => MatrixKind::Plain,
            "pmatrix" => MatrixKind::Paren,
            "bmatrix" => MatrixKind::Bracket,
            "Bmatrix" => MatrixKind::Brace,
            "vmatrix" => MatrixKind::Vert,
            "Vmatrix" => MatrixKind::DoubleVert,
            "cases" => MatrixKind::Cases,
            _ => MatrixKind::Plain,
        };

        let mut rows: Vec<Vec<Vec<MathNode>>> = Vec::new();
        let mut current_row: Vec<Vec<MathNode>> = Vec::new();
        let mut current_cell: Vec<MathNode> = Vec::new();

        let end_marker = format!("\\end{{{}}}", name);

        loop {
            if self.remaining().starts_with(&end_marker) {
                self.pos += end_marker.len();
                break;
            }
            if self.peek().is_none() {
                break;
            }

            // Check for row separator \\
            if self.remaining().starts_with("\\\\") {
                self.pos += 2;
                current_row.push(std::mem::take(&mut current_cell));
                rows.push(std::mem::take(&mut current_row));
                continue;
            }

            // Check for column separator &
            if self.peek() == Some('&') {
                self.advance();
                current_row.push(std::mem::take(&mut current_cell));
                continue;
            }

            if let Some(node) = self.parse_one() {
                let node = self.maybe_attach_scripts(node);
                current_cell.push(node);
            }
        }

        // Push remaining cell/row
        if !current_cell.is_empty() || !current_row.is_empty() {
            current_row.push(current_cell);
        }
        if !current_row.is_empty() {
            rows.push(current_row);
        }

        Some(MathNode::Matrix(kind, rows))
    }
}

fn is_math_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '+' | '-'
                | '*'
                | '/'
                | '='
                | '<'
                | '>'
                | '!'
                | '|'
                | '('
                | ')'
                | '['
                | ']'
                | ','
                | '.'
                | ';'
                | ':'
                | '?'
                | '@'
                | '~'
        )
}

fn operator_name(name: &str) -> Option<&str> {
    match name {
        "sin" => Some("sin"),
        "cos" => Some("cos"),
        "tan" => Some("tan"),
        "cot" => Some("cot"),
        "sec" => Some("sec"),
        "csc" => Some("csc"),
        "arcsin" => Some("arcsin"),
        "arccos" => Some("arccos"),
        "arctan" => Some("arctan"),
        "sinh" => Some("sinh"),
        "cosh" => Some("cosh"),
        "tanh" => Some("tanh"),
        "log" => Some("log"),
        "ln" => Some("ln"),
        "exp" => Some("exp"),
        "lim" => Some("lim"),
        "limsup" => Some("lim sup"),
        "liminf" => Some("lim inf"),
        "sup" => Some("sup"),
        "inf" => Some("inf"),
        "min" => Some("min"),
        "max" => Some("max"),
        "arg" => Some("arg"),
        "det" => Some("det"),
        "dim" => Some("dim"),
        "gcd" => Some("gcd"),
        "hom" => Some("hom"),
        "ker" => Some("ker"),
        "deg" => Some("deg"),
        "Pr" => Some("Pr"),
        "mod" => Some("mod"),
        _ => None,
    }
}

fn symbol_from_name(name: &str) -> Option<Symbol> {
    Some(match name {
        // Greek lowercase
        "alpha" => Symbol::Alpha,
        "beta" => Symbol::Beta,
        "gamma" => Symbol::Gamma,
        "delta" => Symbol::Delta,
        "epsilon" => Symbol::Epsilon,
        "varepsilon" => Symbol::VarEpsilon,
        "zeta" => Symbol::Zeta,
        "eta" => Symbol::Eta,
        "theta" => Symbol::Theta,
        "vartheta" => Symbol::VarTheta,
        "iota" => Symbol::Iota,
        "kappa" => Symbol::Kappa,
        "lambda" => Symbol::Lambda,
        "mu" => Symbol::Mu,
        "nu" => Symbol::Nu,
        "xi" => Symbol::Xi,
        "omicron" => Symbol::Omicron,
        "pi" => Symbol::Pi,
        "varpi" => Symbol::VarPi,
        "rho" => Symbol::Rho,
        "varrho" => Symbol::VarRho,
        "sigma" => Symbol::Sigma,
        "varsigma" => Symbol::VarSigma,
        "tau" => Symbol::Tau,
        "upsilon" => Symbol::Upsilon,
        "phi" => Symbol::Phi,
        "varphi" => Symbol::VarPhi,
        "chi" => Symbol::Chi,
        "psi" => Symbol::Psi,
        "omega" => Symbol::Omega,
        // Greek uppercase
        "Gamma" => Symbol::UpperGamma,
        "Delta" => Symbol::UpperDelta,
        "Theta" => Symbol::UpperTheta,
        "Lambda" => Symbol::UpperLambda,
        "Xi" => Symbol::UpperXi,
        "Pi" => Symbol::UpperPi,
        "Sigma" => Symbol::UpperSigma,
        "Upsilon" => Symbol::UpperUpsilon,
        "Phi" => Symbol::UpperPhi,
        "Psi" => Symbol::UpperPsi,
        "Omega" => Symbol::UpperOmega,
        // Big operators
        "sum" => Symbol::Sum,
        "prod" => Symbol::Prod,
        "coprod" => Symbol::Coprod,
        "int" => Symbol::Int,
        "iint" => Symbol::Iint,
        "iiint" => Symbol::Iiint,
        "oint" => Symbol::Oint,
        "bigcup" => Symbol::Bigcup,
        "bigcap" => Symbol::Bigcap,
        "bigsqcup" => Symbol::Bigsqcup,
        "bigvee" => Symbol::Bigvee,
        "bigwedge" => Symbol::Bigwedge,
        "bigoplus" => Symbol::Bigoplus,
        "bigotimes" => Symbol::Bigotimes,
        "bigodot" => Symbol::Bigodot,
        // Binary operators
        "times" => Symbol::Times,
        "div" => Symbol::Div,
        "cdot" => Symbol::Cdot,
        "star" => Symbol::Star,
        "circ" => Symbol::Circ,
        "bullet" => Symbol::Bullet,
        "diamond" => Symbol::Diamond,
        "pm" => Symbol::Pm,
        "mp" => Symbol::Mp,
        "ast" => Symbol::Ast,
        "dagger" => Symbol::Dagger,
        "ddagger" => Symbol::Ddagger,
        "setminus" => Symbol::Setminus,
        "wr" => Symbol::Wr,
        // Relations
        "neq" | "ne" => Symbol::Neq,
        "leq" | "le" => Symbol::Leq,
        "geq" | "ge" => Symbol::Geq,
        "ll" => Symbol::Ll,
        "gg" => Symbol::Gg,
        "prec" => Symbol::Prec,
        "succ" => Symbol::Succ,
        "preceq" => Symbol::Preceq,
        "succeq" => Symbol::Succeq,
        "sim" => Symbol::Sim,
        "simeq" => Symbol::Simeq,
        "approx" => Symbol::Approx,
        "cong" => Symbol::Cong,
        "equiv" => Symbol::Equiv,
        "subset" => Symbol::Subset,
        "supset" => Symbol::Supset,
        "subseteq" => Symbol::Subseteq,
        "supseteq" => Symbol::Supseteq,
        "in" => Symbol::In,
        "ni" => Symbol::Ni,
        "notin" => Symbol::Notin,
        "propto" => Symbol::Propto,
        "parallel" => Symbol::Parallel,
        "perp" => Symbol::Perp,
        "mid" => Symbol::Mid,
        "vdash" => Symbol::Vdash,
        "dashv" => Symbol::Dashv,
        "models" => Symbol::Models,
        // Arrows
        "leftarrow" | "gets" => Symbol::LeftArrow,
        "rightarrow" | "to" => Symbol::RightArrow,
        "leftrightarrow" => Symbol::LeftRightArrow,
        "uparrow" => Symbol::Uparrow,
        "downarrow" => Symbol::Downarrow,
        "Leftarrow" => Symbol::DoubleLeftArrow,
        "Rightarrow" | "implies" => Symbol::DoubleRightArrow,
        "Leftrightarrow" | "iff" => Symbol::DoubleLeftRightArrow,
        "mapsto" => Symbol::Mapsto,
        "longrightarrow" => Symbol::LongRightArrow,
        "longleftarrow" => Symbol::LongLeftArrow,
        "longleftrightarrow" => Symbol::LongLeftRightArrow,
        "hookrightarrow" => Symbol::Hookrightarrow,
        "hookleftarrow" => Symbol::Hookleftarrow,
        "nearrow" => Symbol::Nearrow,
        "searrow" => Symbol::Searrow,
        "nwarrow" => Symbol::Nwarrow,
        "swarrow" => Symbol::Swarrow,
        // Dots
        "ldots" | "dots" => Symbol::Ldots,
        "cdots" => Symbol::Cdots,
        "vdots" => Symbol::Vdots,
        "ddots" => Symbol::Ddots,
        // Misc
        "infty" => Symbol::Infty,
        "partial" => Symbol::Partial,
        "nabla" => Symbol::Nabla,
        "forall" => Symbol::Forall,
        "exists" => Symbol::Exists,
        "nexists" => Symbol::Nexists,
        "emptyset" | "varnothing" => Symbol::Emptyset,
        "aleph" => Symbol::Aleph,
        "hbar" => Symbol::Hbar,
        "ell" => Symbol::Ell,
        "wp" => Symbol::Wp,
        "Re" => Symbol::Re,
        "Im" => Symbol::Im,
        "angle" => Symbol::Angle,
        "triangle" => Symbol::Triangle,
        "triangledown" => Symbol::Triangledown,
        "neg" | "lnot" => Symbol::Neg,
        "flat" => Symbol::Flat,
        "natural" => Symbol::Natural,
        "sharp" => Symbol::Sharp,
        "clubsuit" => Symbol::Clubsuit,
        "diamondsuit" => Symbol::Diamondsuit,
        "heartsuit" => Symbol::Heartsuit,
        "spadesuit" => Symbol::Spadesuit,
        // Delimiters
        "langle" => Symbol::Langle,
        "rangle" => Symbol::Rangle,
        "lfloor" => Symbol::Lfloor,
        "rfloor" => Symbol::Rfloor,
        "lceil" => Symbol::Lceil,
        "rceil" => Symbol::Rceil,
        "vert" => Symbol::Vert,
        "Vert" | "|" => Symbol::DoubleVert,
        _ => return None,
    })
}

/// Map Symbol to its Unicode codepoint in a math font
pub fn symbol_to_char(sym: Symbol) -> char {
    match sym {
        // Greek lowercase (mathematical italic in Unicode math block)
        Symbol::Alpha => '\u{03B1}',
        Symbol::Beta => '\u{03B2}',
        Symbol::Gamma => '\u{03B3}',
        Symbol::Delta => '\u{03B4}',
        Symbol::Epsilon => '\u{03F5}',
        Symbol::VarEpsilon => '\u{03B5}',
        Symbol::Zeta => '\u{03B6}',
        Symbol::Eta => '\u{03B7}',
        Symbol::Theta => '\u{03B8}',
        Symbol::VarTheta => '\u{03D1}',
        Symbol::Iota => '\u{03B9}',
        Symbol::Kappa => '\u{03BA}',
        Symbol::Lambda => '\u{03BB}',
        Symbol::Mu => '\u{03BC}',
        Symbol::Nu => '\u{03BD}',
        Symbol::Xi => '\u{03BE}',
        Symbol::Omicron => '\u{03BF}',
        Symbol::Pi => '\u{03C0}',
        Symbol::VarPi => '\u{03D6}',
        Symbol::Rho => '\u{03C1}',
        Symbol::VarRho => '\u{03F1}',
        Symbol::Sigma => '\u{03C3}',
        Symbol::VarSigma => '\u{03C2}',
        Symbol::Tau => '\u{03C4}',
        Symbol::Upsilon => '\u{03C5}',
        Symbol::Phi => '\u{03D5}',
        Symbol::VarPhi => '\u{03C6}',
        Symbol::Chi => '\u{03C7}',
        Symbol::Psi => '\u{03C8}',
        Symbol::Omega => '\u{03C9}',
        // Greek uppercase
        Symbol::UpperGamma => '\u{0393}',
        Symbol::UpperDelta => '\u{0394}',
        Symbol::UpperTheta => '\u{0398}',
        Symbol::UpperLambda => '\u{039B}',
        Symbol::UpperXi => '\u{039E}',
        Symbol::UpperPi => '\u{03A0}',
        Symbol::UpperSigma => '\u{03A3}',
        Symbol::UpperUpsilon => '\u{03A5}',
        Symbol::UpperPhi => '\u{03A6}',
        Symbol::UpperPsi => '\u{03A8}',
        Symbol::UpperOmega => '\u{03A9}',
        // Big operators
        Symbol::Sum => '\u{2211}',
        Symbol::Prod => '\u{220F}',
        Symbol::Coprod => '\u{2210}',
        Symbol::Int => '\u{222B}',
        Symbol::Iint => '\u{222C}',
        Symbol::Iiint => '\u{222D}',
        Symbol::Oint => '\u{222E}',
        Symbol::Bigcup => '\u{22C3}',
        Symbol::Bigcap => '\u{22C2}',
        Symbol::Bigsqcup => '\u{2A06}',
        Symbol::Bigvee => '\u{22C1}',
        Symbol::Bigwedge => '\u{22C0}',
        Symbol::Bigoplus => '\u{2A01}',
        Symbol::Bigotimes => '\u{2A02}',
        Symbol::Bigodot => '\u{2A00}',
        // Binary operators
        Symbol::Plus => '+',
        Symbol::Minus => '\u{2212}',
        Symbol::Times => '\u{00D7}',
        Symbol::Div => '\u{00F7}',
        Symbol::Cdot => '\u{22C5}',
        Symbol::Star => '\u{22C6}',
        Symbol::Circ => '\u{2218}',
        Symbol::Bullet => '\u{2219}',
        Symbol::Diamond => '\u{22C4}',
        Symbol::Pm => '\u{00B1}',
        Symbol::Mp => '\u{2213}',
        Symbol::Ast => '\u{2217}',
        Symbol::Dagger => '\u{2020}',
        Symbol::Ddagger => '\u{2021}',
        Symbol::Setminus => '\u{2216}',
        Symbol::Wr => '\u{2240}',
        // Relations
        Symbol::Eq => '=',
        Symbol::Neq => '\u{2260}',
        Symbol::Lt => '<',
        Symbol::Gt => '>',
        Symbol::Le | Symbol::Leq => '\u{2264}',
        Symbol::Ge | Symbol::Geq => '\u{2265}',
        Symbol::Ll => '\u{226A}',
        Symbol::Gg => '\u{226B}',
        Symbol::Prec => '\u{227A}',
        Symbol::Succ => '\u{227B}',
        Symbol::Preceq => '\u{2AAF}',
        Symbol::Succeq => '\u{2AB0}',
        Symbol::Sim => '\u{223C}',
        Symbol::Simeq => '\u{2243}',
        Symbol::Approx => '\u{2248}',
        Symbol::Cong => '\u{2245}',
        Symbol::Equiv => '\u{2261}',
        Symbol::Subset => '\u{2282}',
        Symbol::Supset => '\u{2283}',
        Symbol::Subseteq => '\u{2286}',
        Symbol::Supseteq => '\u{2287}',
        Symbol::In => '\u{2208}',
        Symbol::Ni => '\u{220B}',
        Symbol::Notin => '\u{2209}',
        Symbol::Propto => '\u{221D}',
        Symbol::Parallel => '\u{2225}',
        Symbol::Perp => '\u{22A5}',
        Symbol::Mid => '\u{2223}',
        Symbol::Vdash => '\u{22A2}',
        Symbol::Dashv => '\u{22A3}',
        Symbol::Models => '\u{22A8}',
        // Arrows
        Symbol::LeftArrow => '\u{2190}',
        Symbol::RightArrow => '\u{2192}',
        Symbol::LeftRightArrow => '\u{2194}',
        Symbol::Uparrow => '\u{2191}',
        Symbol::Downarrow => '\u{2193}',
        Symbol::DoubleLeftArrow => '\u{21D0}',
        Symbol::DoubleRightArrow => '\u{21D2}',
        Symbol::DoubleLeftRightArrow => '\u{21D4}',
        Symbol::Mapsto => '\u{21A6}',
        Symbol::LongRightArrow => '\u{27F6}',
        Symbol::LongLeftArrow => '\u{27F5}',
        Symbol::LongLeftRightArrow => '\u{27F7}',
        Symbol::Hookrightarrow => '\u{21AA}',
        Symbol::Hookleftarrow => '\u{21A9}',
        Symbol::Nearrow => '\u{2197}',
        Symbol::Searrow => '\u{2198}',
        Symbol::Nwarrow => '\u{2196}',
        Symbol::Swarrow => '\u{2199}',
        // Dots
        Symbol::Ldots => '\u{2026}',
        Symbol::Cdots => '\u{22EF}',
        Symbol::Vdots => '\u{22EE}',
        Symbol::Ddots => '\u{22F1}',
        // Misc
        Symbol::Infty => '\u{221E}',
        Symbol::Partial => '\u{2202}',
        Symbol::Nabla => '\u{2207}',
        Symbol::Forall => '\u{2200}',
        Symbol::Exists => '\u{2203}',
        Symbol::Nexists => '\u{2204}',
        Symbol::Emptyset => '\u{2205}',
        Symbol::Aleph => '\u{2135}',
        Symbol::Hbar => '\u{210F}',
        Symbol::Ell => '\u{2113}',
        Symbol::Wp => '\u{2118}',
        Symbol::Re => '\u{211C}',
        Symbol::Im => '\u{2111}',
        Symbol::Angle => '\u{2220}',
        Symbol::Triangle => '\u{25B3}',
        Symbol::Triangledown => '\u{25BD}',
        Symbol::Neg => '\u{00AC}',
        Symbol::Flat => '\u{266D}',
        Symbol::Natural => '\u{266E}',
        Symbol::Sharp => '\u{266F}',
        Symbol::Clubsuit => '\u{2663}',
        Symbol::Diamondsuit => '\u{2662}',
        Symbol::Heartsuit => '\u{2661}',
        Symbol::Spadesuit => '\u{2660}',
        // Delimiters
        Symbol::Langle => '\u{27E8}',
        Symbol::Rangle => '\u{27E9}',
        Symbol::Lfloor => '\u{230A}',
        Symbol::Rfloor => '\u{230B}',
        Symbol::Lceil => '\u{2308}',
        Symbol::Rceil => '\u{2309}',
        Symbol::Vert => '|',
        Symbol::DoubleVert => '\u{2016}',
        // Punctuation
        Symbol::Comma => ',',
        Symbol::Semicolon => ';',
        Symbol::Colon => ':',
    }
}

/// Parse a LaTeX math string into an AST.
/// The input should be the content between $ ... $ (without the dollar signs).
pub fn parse(input: &str) -> Vec<MathNode> {
    let mut parser = Parser::new(input);
    parser.parse_until(|p| p.peek().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_chars() {
        let nodes = parse("x+y");
        assert_eq!(
            nodes,
            vec![
                MathNode::Char('x'),
                MathNode::Char('+'),
                MathNode::Char('y'),
            ]
        );
    }

    #[test]
    fn test_frac() {
        let nodes = parse(r"\frac{a}{b}");
        assert_eq!(
            nodes,
            vec![MathNode::Frac(
                vec![MathNode::Char('a')],
                vec![MathNode::Char('b')],
            ),]
        );
    }

    #[test]
    fn test_superscript() {
        let nodes = parse("x^2");
        assert_eq!(
            nodes,
            vec![MathNode::Group(vec![
                MathNode::Char('x'),
                MathNode::Superscript(vec![MathNode::Char('2')]),
            ]),]
        );
    }

    #[test]
    fn test_subscript() {
        let nodes = parse("x_i");
        assert_eq!(
            nodes,
            vec![MathNode::Group(vec![
                MathNode::Char('x'),
                MathNode::Subscript(vec![MathNode::Char('i')]),
            ]),]
        );
    }

    #[test]
    fn test_sub_superscript() {
        let nodes = parse("x_i^2");
        assert_eq!(
            nodes,
            vec![MathNode::Group(vec![
                MathNode::Char('x'),
                MathNode::SubSuperscript(vec![MathNode::Char('i')], vec![MathNode::Char('2')],),
            ]),]
        );
    }

    #[test]
    fn test_sqrt() {
        let nodes = parse(r"\sqrt{x}");
        assert_eq!(
            nodes,
            vec![MathNode::Sqrt(None, vec![MathNode::Char('x')]),]
        );
    }

    #[test]
    fn test_sqrt_with_index() {
        let nodes = parse(r"\sqrt[3]{x}");
        assert_eq!(
            nodes,
            vec![MathNode::Sqrt(
                Some(vec![MathNode::Char('3')]),
                vec![MathNode::Char('x')],
            ),]
        );
    }

    #[test]
    fn test_greek() {
        let nodes = parse(r"\alpha + \beta");
        assert_eq!(
            nodes,
            vec![
                MathNode::Symbol(Symbol::Alpha),
                MathNode::Char('+'),
                MathNode::Symbol(Symbol::Beta),
            ]
        );
    }

    #[test]
    fn test_accent() {
        let nodes = parse(r"\hat{x}");
        assert_eq!(
            nodes,
            vec![MathNode::Accent(AccentKind::Hat, vec![MathNode::Char('x')]),]
        );
    }

    #[test]
    fn test_nested_frac() {
        let nodes = parse(r"\frac{\frac{a}{b}}{c}");
        assert_eq!(
            nodes,
            vec![MathNode::Frac(
                vec![MathNode::Frac(
                    vec![MathNode::Char('a')],
                    vec![MathNode::Char('b')],
                )],
                vec![MathNode::Char('c')],
            ),]
        );
    }

    #[test]
    fn test_operator() {
        let nodes = parse(r"\sin x");
        assert_eq!(
            nodes,
            vec![
                MathNode::OperatorName("sin".to_string()),
                MathNode::Char('x'),
            ]
        );
    }

    #[test]
    fn test_left_right() {
        let nodes = parse(r"\left(\frac{a}{b}\right)");
        assert_eq!(
            nodes,
            vec![MathNode::LeftRight(
                Delimiter::Paren,
                vec![MathNode::Frac(
                    vec![MathNode::Char('a')],
                    vec![MathNode::Char('b')],
                )],
                Delimiter::Paren,
            ),]
        );
    }

    #[test]
    fn test_matrix() {
        let nodes = parse(r"\begin{pmatrix}a & b \\ c & d\end{pmatrix}");
        assert_eq!(
            nodes,
            vec![MathNode::Matrix(
                MatrixKind::Paren,
                vec![
                    vec![vec![MathNode::Char('a')], vec![MathNode::Char('b')],],
                    vec![vec![MathNode::Char('c')], vec![MathNode::Char('d')],],
                ],
            ),]
        );
    }

    #[test]
    fn test_text() {
        let nodes = parse(r"\text{hello}");
        assert_eq!(nodes, vec![MathNode::Text("hello".to_string()),]);
    }

    #[test]
    fn test_sum_with_limits() {
        let nodes = parse(r"\sum_{i=1}^{n}");
        assert_eq!(
            nodes,
            vec![MathNode::Group(vec![
                MathNode::Symbol(Symbol::Sum),
                MathNode::SubSuperscript(
                    vec![
                        MathNode::Char('i'),
                        MathNode::Char('='),
                        MathNode::Char('1')
                    ],
                    vec![MathNode::Char('n')],
                ),
            ]),]
        );
    }

    #[test]
    fn test_spacing() {
        let nodes = parse(r"a\,b\;c\quad d");
        assert_eq!(
            nodes,
            vec![
                MathNode::Char('a'),
                MathNode::Space(SpaceWidth::Thin),
                MathNode::Char('b'),
                MathNode::Space(SpaceWidth::Thick),
                MathNode::Char('c'),
                MathNode::Space(SpaceWidth::Quad),
                MathNode::Char('d'),
            ]
        );
    }

    #[test]
    fn test_mathbf() {
        let nodes = parse(r"\mathbf{x}");
        assert_eq!(
            nodes,
            vec![MathNode::MathVariant(
                MathVariant::Bold,
                vec![MathNode::Char('x')]
            ),]
        );
    }

    #[test]
    fn test_complex_expression() {
        // E = mc^2
        let nodes = parse(r"E = mc^2");
        assert_eq!(
            nodes,
            vec![
                MathNode::Char('E'),
                MathNode::Char('='),
                MathNode::Char('m'),
                MathNode::Group(vec![
                    MathNode::Char('c'),
                    MathNode::Superscript(vec![MathNode::Char('2')]),
                ]),
            ]
        );
    }

    #[test]
    fn test_braces_as_delimiters() {
        let nodes = parse(r"\left\{x\right\}");
        assert_eq!(
            nodes,
            vec![MathNode::LeftRight(
                Delimiter::Brace,
                vec![MathNode::Char('x')],
                Delimiter::Brace,
            ),]
        );
    }

    #[test]
    fn test_overline() {
        let nodes = parse(r"\overline{AB}");
        assert_eq!(
            nodes,
            vec![MathNode::Overline(vec![
                MathNode::Char('A'),
                MathNode::Char('B')
            ]),]
        );
    }

    #[test]
    fn test_color() {
        let nodes = parse(r"\color{red}{x+y}");
        assert_eq!(
            nodes,
            vec![MathNode::Color(
                "red".to_string(),
                vec![
                    MathNode::Char('x'),
                    MathNode::Char('+'),
                    MathNode::Char('y')
                ],
            ),]
        );
    }
}
