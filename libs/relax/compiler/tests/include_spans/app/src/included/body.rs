// Lines and columns must belong to this file, not main.rs's include! call.

const DIRECT: &str = spans!(
    use mod prelude;
    café r#type "é" + (nested [42])
);

forward! {
    const FORWARDED: &str = spans!(
        two   words
        { more tokens }
    );
}

const CALL: (usize, usize) = call_location!();
const LOCATION: (u32, u32) = (line!(), column!());
fn included_file() -> &'static str { authored_file!(here) }
const TEXT: &str = include_str!("text.txt");
include!("nested.rs");

macro_rules! relay {
    ($($token:tt)*) => { spans!($($token)*) };
}
const RULES: &str = relay!(through   rules);
const EXPRESSION: &str = include!("expression.rs");

#[include_spans_mac::attribute_spans]
struct Attribute;

#[derive(include_spans_mac::DeriveSpans)]
struct Derived;
