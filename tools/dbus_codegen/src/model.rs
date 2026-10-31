use crate::xml::Document;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Type {
    Basic(char),
    Array(Box<Type>),
    Struct(Vec<Type>),
    Dict(Box<Type>, Box<Type>),
}
impl Type {
    pub fn parse(s: &str) -> Result<Self, String> {
        fn take(s: &mut &[u8], dict: bool) -> Result<Type, String> {
            let (&c, rest) = s.split_first().ok_or("incomplete signature")?;
            *s = rest;
            Ok(match c {
                b'a' => Type::Array(Box::new(take(s, true)?)),
                b'(' => {
                    let mut fields = Vec::new();
                    while s.first() != Some(&b')') {
                        fields.push(take(s, false)?);
                    }
                    *s = &s[1..];
                    if fields.is_empty() {
                        return Err("empty struct".into());
                    }
                    Type::Struct(fields)
                }
                b'{' if dict => {
                    let key = take(s, false)?;
                    if !matches!(key, Type::Basic(c) if c != 'v') {
                        return Err("invalid dictionary key".into());
                    }
                    let value = take(s, false)?;
                    if s.first() != Some(&b'}') {
                        return Err("unclosed dictionary".into());
                    }
                    *s = &s[1..];
                    Type::Dict(Box::new(key), Box::new(value))
                }
                c if b"ybnqiuxtdsogv".contains(&c) => Type::Basic(c as char),
                _ => return Err(format!("unsupported signature code {}", c as char)),
            })
        }
        let mut bytes = s.as_bytes();
        let ty = take(&mut bytes, false)?;
        if !bytes.is_empty() {
            return Err(format!("expected one type: {s}"));
        }
        Ok(ty)
    }
    pub fn sig(&self) -> String {
        match self {
            Self::Basic(c) => c.to_string(),
            Self::Array(t) => format!("a{}", t.sig()),
            Self::Struct(ts) => format!("({})", ts.iter().map(Type::sig).collect::<String>()),
            Self::Dict(k, v) => format!("{{{}{}}}", k.sig(), v.sig()),
        }
    }
    pub fn owned(&self) -> String {
        if self.sig() == "(so)" {
            return "ObjRef".into();
        }
        match self {
            Self::Basic(c) => match c {
                'y' => "u8",
                'b' => "bool",
                'n' => "i16",
                'q' => "u16",
                'i' => "i32",
                'u' => "u32",
                'x' => "i64",
                't' => "u64",
                'd' => "f64",
                's' | 'o' | 'g' => "String",
                'v' => "Value",
                _ => unreachable!(),
            }
            .into(),
            Self::Array(t) => format!("Vec<{}>", t.owned()),
            Self::Struct(ts) => tuple(&ts.iter().map(Type::owned).collect::<Vec<_>>()),
            Self::Dict(k, v) => tuple(&[k.owned(), v.owned()]),
        }
    }
    pub fn input(&self) -> String {
        match self {
            Self::Basic('s' | 'o' | 'g') => "&str".into(),
            Self::Basic('v') => "&Value".into(),
            Self::Basic(_) => self.owned(),
            Self::Array(t) => format!("&[{}]", t.owned()),
            _ => format!("&{}", self.owned()),
        }
    }
    pub fn borrow(&self, expr: &str) -> String {
        if matches!(self, Self::Basic(c) if !"sogv".contains(*c)) {
            expr.into()
        } else {
            format!("&({expr})")
        }
    }
    pub fn codec(&self) -> &'static str {
        match self {
            Self::Basic(c) => match c {
                'y' => "byte",
                'b' => "bool",
                'n' => "i16",
                'q' => "u16",
                'i' => "i32",
                'u' => "u32",
                'x' => "i64",
                't' => "u64",
                'd' => "f64",
                's' => "str",
                'o' => "object_path",
                'g' => "signature",
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}
pub fn tuple(parts: &[String]) -> String {
    format!(
        "({}{})",
        parts.join(", "),
        if parts.len() == 1 { "," } else { "" }
    )
}
pub fn snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !result.ends_with('_') {
                result.push('_');
            }
            continue;
        }
        if c.is_ascii_uppercase()
            && i > 0
            && chars[i - 1].is_ascii_alphanumeric()
            && (chars[i - 1].is_ascii_lowercase()
                || chars[i - 1].is_ascii_digit()
                || chars.get(i + 1).is_some_and(char::is_ascii_lowercase))
        {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    result
}
pub fn camel(s: &str) -> String {
    snake(s)
        .split('_')
        .filter(|s| !s.is_empty())
        .map(|s| format!("{}{}", s[..1].to_ascii_uppercase(), &s[1..]))
        .collect()
}
pub type Annotations = Vec<(String, String)>;
pub struct Arg {
    pub name: String,
    pub ty: Type,
    pub output: bool,
    pub annotations: Annotations,
}
pub struct Member {
    pub name: String,
    pub args: Vec<Arg>,
    pub annotations: Annotations,
}
pub struct Property {
    pub name: String,
    pub ty: Type,
    pub access: String,
    pub annotations: Annotations,
}
pub struct Interface {
    pub name: String,
    pub methods: Vec<Member>,
    pub signals: Vec<Member>,
    pub properties: Vec<Property>,
    pub annotations: Annotations,
}
impl Interface {
    pub fn short(&self) -> &str {
        self.name
            .strip_prefix("org.a11y.atspi.")
            .unwrap_or(&self.name)
    }
    pub fn id(&self) -> String {
        snake(self.short())
    }
    pub fn server(&self) -> String {
        format!("{}Server", camel(self.short()))
    }
}
fn annotations(doc: &Document, id: usize) -> Annotations {
    doc.children(id, "annotation")
        .into_iter()
        .map(|i| {
            let e = &doc.nodes[i];
            (e.attr("name").into(), e.attr("value").into())
        })
        .collect()
}
pub fn parse(source: &str) -> Result<Vec<Interface>, String> {
    let doc = Document::parse(source)?;
    let root = doc.child(0, "node").ok_or("missing node")?;
    doc.children(root, "interface")
        .into_iter()
        .map(|id| {
            let members = |kind: &str| -> Result<Vec<Member>, String> {
                doc.children(id, kind)
                    .into_iter()
                    .map(|m| {
                        let args = doc
                            .children(m, "arg")
                            .into_iter()
                            .map(|a| {
                                let e = &doc.nodes[a];
                                let direction = e.attr("direction");
                                if !matches!(direction, "" | "in" | "out") {
                                    return Err("invalid argument direction".into());
                                }
                                Ok(Arg {
                                    name: e.attr("name").into(),
                                    ty: Type::parse(e.attr("type"))?,
                                    output: direction == "out"
                                        || (direction.is_empty() && kind == "signal"),
                                    annotations: annotations(&doc, a),
                                })
                            })
                            .collect::<Result<_, String>>()?;
                        Ok(Member {
                            name: doc.nodes[m].attr("name").into(),
                            args,
                            annotations: annotations(&doc, m),
                        })
                    })
                    .collect()
            };
            let properties = doc
                .children(id, "property")
                .into_iter()
                .map(|p| {
                    let e = &doc.nodes[p];
                    if !matches!(e.attr("access"), "read" | "write" | "readwrite") {
                        return Err("invalid property access".into());
                    }
                    Ok(Property {
                        name: e.attr("name").into(),
                        ty: Type::parse(e.attr("type"))?,
                        access: e.attr("access").into(),
                        annotations: annotations(&doc, p),
                    })
                })
                .collect::<Result<_, String>>()?;
            Ok(Interface {
                name: doc.nodes[id].attr("name").into(),
                methods: members("method")?,
                signals: members("signal")?,
                properties,
                annotations: annotations(&doc, id),
            })
        })
        .collect()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn emit_annotations(out: &mut String, annotations: &Annotations) {
    for (name, value) in annotations {
        out.push_str(&format!(
            "<annotation name=\"{}\" value=\"{}\"/>",
            escape(name),
            escape(value)
        ));
    }
}
pub fn introspection(i: &Interface) -> String {
    let mut out = format!("<interface name=\"{}\">", escape(&i.name));
    emit_annotations(&mut out, &i.annotations);
    for (kind, members) in [("method", &i.methods), ("signal", &i.signals)] {
        for m in members {
            out.push_str(&format!("<{kind} name=\"{}\">", escape(&m.name)));
            for a in &m.args {
                out.push_str(&format!(
                    "<arg type=\"{}\" direction=\"{}\"",
                    escape(&a.ty.sig()),
                    if a.output { "out" } else { "in" }
                ));
                if !a.name.is_empty() {
                    out.push_str(&format!(" name=\"{}\"", escape(&a.name)));
                }
                out.push('>');
                emit_annotations(&mut out, &a.annotations);
                out.push_str("</arg>");
            }
            emit_annotations(&mut out, &m.annotations);
            out.push_str(&format!("</{kind}>"));
        }
    }
    for p in &i.properties {
        out.push_str(&format!(
            "<property name=\"{}\" type=\"{}\" access=\"{}\">",
            escape(&p.name),
            escape(&p.ty.sig()),
            p.access
        ));
        emit_annotations(&mut out, &p.annotations);
        out.push_str("</property>");
    }
    out.push_str("</interface>");
    out
}
