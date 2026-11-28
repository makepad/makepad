//! The Splash value subset accepted from a structural editing peer.
use super::{parse_value, write, Kind, Node};
use crate::tokenizer::{lex, ScriptTokenTag};
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiteralClassError {
    pub span: Range<usize>,
    pub reason: String,
}

const SAFE_VALUE_CALLS: &[&str] = &["vec2", "vec3", "vec4", "rgb", "rgba", "hsl", "hsla", "dvec2", "inset"];

fn is_allowed_path(path: &str) -> bool {
    write::is_path(path) && path.split('.').next() != Some("scope")
}

// A bracket that starts a separate AST item can still continue the preceding
// expression in the VM. Only lex the intervening trivia and the opening token.
fn check_no_unseparated_postfix<'a>(
    pairs: impl Iterator<Item = (&'a Range<usize>, &'a Range<usize>)>,
    src: &str,
    allow_newline_break: bool,
    allow_space_array: bool,
) -> Result<(), LiteralClassError> {
    for (prev, next) in pairs {
        let Some(&opener @ (b'(' | b'[' | b'{')) = src.as_bytes().get(next.start) else { continue };
        let tokens = lex(&src[prev.end..next.start + 1]);
        if let Some((last, gap)) = tokens.split_last() {
            if gap.iter().any(|t| t.token().kind() == ScriptTokenTag::Separator)
                || (allow_newline_break && last.preceded_by_newline())
                || (allow_space_array && opener == b'[' && last.preceded_by_space()) {
                continue;
            }
        }
        return Err(LiteralClassError { span: next.start..next.start + 1, reason: "unseparated postfix expression".into() });
    }
    Ok(())
}

pub fn check_literal_class(node: &Node, src: &str) -> Result<(), LiteralClassError> {
    let check = |child: &Node| check_literal_class(child, src);
    let allowed = match &node.kind {
        Kind::Number(_) | Kind::Str(_) | Kind::Name(_) | Kind::Color(_) => true,
        Kind::Ident(path) => is_allowed_path(path),
        Kind::Member { base, name, .. } if write::is_ident(name) => return check(base),
        Kind::Unary { op, operand } if matches!(op.as_str(), "-" | "+" | "!") => return check(operand),
        Kind::Chain { first, rest } => {
            check(first)?;
            for link in rest {
                if !matches!(link.op.as_str(), "+" | "-" | "*" | "/") {
                    return Err(LiteralClassError { span: link.op_span.clone(), reason: format!("unsupported operator {}", link.op) });
                }
                check(&link.operand)?;
            }
            true
        }
        Kind::Paren { items, .. } => match items.as_slice() {
            [item] => return check(item),
            _ => false,
        },
        Kind::Array { items, .. } => {
            check_no_unseparated_postfix(items.windows(2).map(|p| (&p[0].span, &p[1].span)), src, false, true)?;
            return items.iter().try_for_each(check);
        }
        Kind::Call { callee, args, .. } if callee.ident().is_some_and(|id| SAFE_VALUE_CALLS.contains(&id)) => {
            check_no_unseparated_postfix(args.windows(2).map(|p| (&p[0].span, &p[1].span)), src, false, false)?;
            for arg in args {
                match &arg.kind {
                    Kind::Entry { name, op, value, .. } if op == ":" && write::is_ident(name) && name != "scope" => check(value)?,
                    _ => check(arg)?,
                }
            }
            true
        }
        Kind::Object { ty, fields, .. } => {
            if ty.as_deref().is_some_and(|path| !is_allowed_path(path)) {
                false
            } else {
                let pairs = fields.windows(2).filter(|p| p[1].name.is_none()).map(|p| (&p[0].span, &p[1].span));
                check_no_unseparated_postfix(pairs, src, true, false)?;
                for field in fields {
                    if let Some(name) = &field.name {
                        let quoted = src.get(field.name_span.clone()).is_some_and(|s| s.starts_with(['"', '\'']));
                        if !quoted && !is_allowed_path(name) {
                            return Err(LiteralClassError { span: field.name_span.clone(), reason: format!("unsupported field name {name}") });
                        }
                    }
                    if !matches!(field.op.as_str(), "" | ":" | ":=" | "+:") {
                        return Err(LiteralClassError { span: field.span.clone(), reason: format!("unsupported field operator {}", field.op) });
                    }
                    check(&field.value)?;
                }
                true
            }
        }
        Kind::Call { .. } | Kind::Member { .. } | Kind::Unary { .. } | Kind::Index { .. } | Kind::Ok { .. }
        | Kind::Entry { .. } | Kind::Error | Kind::Fn { .. } | Kind::Block(_) | Kind::Use { .. }
        | Kind::Let { .. } | Kind::Assign { .. } | Kind::If { .. } | Kind::For { .. }
        | Kind::While { .. } | Kind::Loop { .. } | Kind::Match { .. } | Kind::Try { .. }
        | Kind::Jump { .. } => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(LiteralClassError { span: node.span.clone(), reason: format!("{} is outside the literal class", node.kind_name()) })
    }
}

pub fn parse_literal_class(text: &str) -> Result<Node, String> {
    let node = parse_value(text).map_err(|e| e.to_string())?;
    check_literal_class(&node, text).map_err(|e| format!("{} at byte {}", e.reason, e.span.start))?;
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_class_allows_visual_values_and_rejects_code() {
        for text in ["8", "\"Save\"", "#1a2b3cff", "@ease", "true", "false", "nil",
            "vec4(1, 2, 3, 4)", "inset(top: 8)", "Inset{left: 4}", "theme.color_bg",
            "View { child := Button { text: \"ok\" } }", "[(-2 + 3) * 4, !false]",
            "[1, (2 + 3)]", "[[1 2] [3 4]]", "vec2(1, (2 + 3))", "View { a: 1\n (2) }",
            "View { \"k\": 1 }", "View { 'scope.x': 1 }", "View { draw_bg.color: #fff }"] {
            assert!(parse_literal_class(text).is_ok(), "visual value should pass: {text}");
        }
        for text in ["fn() {}", "crate_resource(\"x\")", "load_dependency(\"x\")",
            "let x = 1", "#(0)", "@(0)", "if true {}", "scope.foo", "mod.std.*", "a[0]",
            "a: 8", "foo.method()", "vec2(1, bad())", "{pixel: fn() {}}", "1 == 1", "~3",
            "View { crate_resource (\"x\") }", "View { a: crate_resource (\"x\") }",
            "View { a [0] }", "View { a { b: 1 } }", "[crate_resource (\"x\")]", "[crate_resource\n(\"x\")]",
            "(crate_resource (\"x\"))", "(crate_resource\n(\"x\"))", "vec2(crate_resource (\"x\"))",
            "vec2(crate_resource\n(\"x\"))", "scope{}", "scope.foo{}", "View { scope.x: 1 }",
            "View { a.*: 1 }", "inset(scope.x: 8)", "inset(a.b: 8)", "inset(scope: 8)"] {
            assert!(parse_literal_class(text).is_err(), "executable or unsupported value should fail: {text}");
        }
    }
}
