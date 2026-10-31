//! Settings cards: a tool call that makes something is shown as a card
//! instead of running. The card is the tool's arguments as prefilled
//! fields (a dropdown for an enum or a boolean, a text line for a string,
//! a number or a list of strings; a free-form map the request prefilled gets
//! a field per key, typed by the prefilled value), the person edits them, and
//! Confirm runs the tool with the values.
//!
//! Everything here is widget-free and generic over a JSON schema:
//! [`Card::for_schema`] builds the card from a tool's argument schema, the
//! optional [`CardLayout`] and the model's (or a chip's) prefill;
//! [`Card::values`] and [`Card::args`] go between the card and the tool's
//! arguments; [`validate_args`] checks edited arguments against the schema
//! before anything runs. The panel draws a [`Card`] and hands back
//! [`CardValues`].

use crate::wire::CardLayout;
use makepad_strict_json::Value;

/// The most fields a card shows. Fields beyond it, and arguments the card
/// cannot edit (objects with declared properties, lists of non-strings), travel as fixed arguments.
pub const MAX_FIELDS: usize = 12;

/// The confirm label when a layout names none.
pub const DEFAULT_CONFIRM: &str = "Make it";

/// One field of a card.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    /// A dropdown. An option whose value is `None` leaves the argument out
    /// (the leading "default" of an optional enum or boolean).
    Pick { arg: String, label: String, options: Vec<(String, Option<Value>)>, selected: usize },
    /// An editable text line.
    Line { arg: String, label: String, hint: String, value: String },
}

impl Field {
    pub fn arg(&self) -> &str {
        match self {
            Field::Pick { arg, .. } | Field::Line { arg, .. } => arg,
        }
    }
}

/// A card for one tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub tool: String,
    pub title: String,
    pub fields: Vec<Field>,
    /// The confirm button's label.
    pub confirm: String,
    /// What the tool does, in a sentence.
    pub why: String,
    /// Prefilled arguments the card does not show, passed through unchanged.
    pub fixed: Vec<(String, Value)>,
    /// Why the last confirm was refused; the card stays up with it.
    pub problem: Option<String>,
    /// Per expanded free-form map: its argument and the prefilled entries
    /// the card has no field for (nested objects, other arrays, keys past
    /// the field limit), kept inside the rebuilt object.
    maps: Vec<(String, Vec<(String, Value)>)>,
    /// The tool's argument schema, which types the lines; an expanded map
    /// carries the types of its keys.
    schema: Value,
}

/// What the person set: the selected option per `Pick` and the text per
/// `Line`, each in field order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardValues {
    pub picks: Vec<usize>,
    pub lines: Vec<String>,
}

/// `gen_run` -> `Gen run`.
pub fn humanise(name: &str) -> String {
    let mut text = name.replace('_', " ");
    if let Some(first) = text.get(..1) {
        let upper = first.to_uppercase();
        text.replace_range(..1, &upper);
    }
    text
}

fn obj_of(type_name: &str) -> Value {
    Value::Obj(vec![("type".into(), Value::Str(type_name.into()))])
}

/// The first sentence of a tool description.
pub fn first_sentence(text: &str) -> String {
    let mut from = 0;
    while let Some(at) = text[from..].find(". ") {
        let end = from + at;
        let before = &text[..end];
        if before.ends_with("e.g") || before.ends_with("i.e") {
            from = end + 2;
            continue;
        }
        return text[..=end].to_string();
    }
    text.trim().to_string()
}

fn type_names(property: &Value) -> Vec<&str> {
    match property.get("type") {
        Some(Value::Str(name)) => vec![name.as_str()],
        Some(Value::Arr(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn option_label(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        other => other.to_json(),
    }
}

/// A value as the text of a line.
fn line_text(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_json(),
    }
}

enum Shape {
    Pick(Vec<Value>),
    Line,
    Fixed,
}

fn is_string_array(property: &Value) -> bool {
    type_names(property) == ["array"]
        && property.get("items").is_some_and(|items| items.get("enum").is_none() && type_names(items) == ["string"])
}

/// A free-form map (`{"type": "object"}` with no declared properties). When
/// the request prefilled it, the card shows one field per key (named
/// `map.key`), typed by the prefilled value; otherwise it is a fixed argument.
fn is_free_map(property: &Value) -> bool {
    type_names(property) == ["object"] && !matches!(property.get("properties"), Some(Value::Obj(declared)) if !declared.is_empty())
}

fn shape_of(property: &Value) -> Shape {
    if let Some(Value::Arr(allowed)) = property.get("enum") {
        if !allowed.is_empty() {
            return Shape::Pick(allowed.clone());
        }
    }
    if property.get("anyOf").is_some() {
        return Shape::Fixed;
    }
    let types = type_names(property);
    if types == ["boolean"] {
        return Shape::Pick(vec![Value::Bool(false), Value::Bool(true)]);
    }
    if !types.is_empty() && types.iter().all(|name| ["string", "number", "integer", "null"].contains(name)) {
        return Shape::Line;
    }
    if is_string_array(property) {
        return Shape::Line;
    }
    Shape::Fixed
}

/// The text of a line as the value its property wants: an integer, a
/// number or a string, in the order the property's types allow. Text that
/// fits none stays a string, which [`validate_args`] then refuses.
fn typed_line(property: &Value, text: &str) -> Value {
    let types = type_names(property);
    let trimmed = text.trim();
    if types.contains(&"integer") {
        if let Ok(n) = trimmed.parse::<i64>() {
            return Value::Int(n);
        }
    }
    if types.contains(&"number") {
        if let Ok(n) = trimmed.parse::<i64>() {
            return Value::Int(n);
        }
        if let Ok(n) = trimmed.parse::<f64>() {
            if n.is_finite() {
                return Value::F64(n);
            }
        }
    }
    if types.contains(&"null") && trimmed == "null" && !types.contains(&"string") {
        return Value::Null;
    }
    Value::Str(text.to_string())
}

impl Card {
    /// The card for a tool whose arguments follow `schema` (JSON text),
    /// filled from `prefill` (an object of arguments the person asked for;
    /// unknown names and nulls are dropped).
    pub fn for_schema(tool: &str, description: &str, schema: &str, layout: Option<&CardLayout>, prefill: &Value) -> Result<Card, String> {
        let mut input = makepad_strict_json::parse(schema.as_bytes()).map_err(|e| format!("the schema of `{tool}` does not parse: {e}"))?;
        let properties: &[(String, Value)] = match input.get("properties") {
            Some(Value::Obj(properties)) => properties,
            _ => &[],
        };
        let required: Vec<&str> = input.get("required").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect();
        let given = |arg: &str| -> Option<&Value> {
            match prefill {
                Value::Obj(fields) => fields.iter().find(|(name, _)| name == arg).map(|(_, value)| value).filter(|value| !value.is_null()),
                _ => None,
            }
        };
        let label_of = |arg: &str| layout.and_then(|l| l.order.iter().find(|f| f.arg == arg)).map(|f| f.label.clone());

        // The layout's order, then the other required fields, then the rest.
        let mut order: Vec<&str> = Vec::new();
        if let Some(layout) = layout {
            for field in &layout.order {
                if properties.iter().any(|(name, _)| *name == field.arg) && !order.contains(&field.arg.as_str()) {
                    order.push(&field.arg);
                }
            }
        }
        for pass in [true, false] {
            for (name, _) in properties {
                if !order.contains(&name.as_str()) && required.contains(&name.as_str()) == pass {
                    order.push(name);
                }
            }
        }

        let mut fields = Vec::new();
        let mut fixed = Vec::new();
        let mut maps: Vec<(String, Vec<(String, Value)>)> = Vec::new();
        let mut typed_maps: Vec<(String, Vec<(String, Value)>)> = Vec::new();
        for arg in order {
            let property = &properties.iter().find(|(name, _)| name == arg).expect("ordered from the properties").1;
            let value = given(arg);
            let hidden = layout.is_some_and(|l| l.hide.iter().any(|h| h == arg));
            let label = label_of(arg).unwrap_or_else(|| humanise(arg));
            let is_required = required.contains(&arg);
            let keep_fixed = |fixed: &mut Vec<(String, Value)>| {
                if let Some(value) = value {
                    fixed.push((arg.to_string(), value.clone()));
                }
            };
            if hidden {
                keep_fixed(&mut fixed);
                continue;
            }
            if is_free_map(property) {
                match value {
                    Some(Value::Obj(entries)) if !entries.is_empty() => {
                        let (mut inner_fixed, mut types) = (Vec::new(), Vec::new());
                        for (key, entry) in entries {
                            let label = humanise(key);
                            let name = format!("{arg}.{key}");
                            let room = fields.len() < MAX_FIELDS;
                            match entry {
                                Value::Bool(on) if room => {
                                    let options = vec![("no".to_string(), Some(Value::Bool(false))), ("yes".to_string(), Some(Value::Bool(true)))];
                                    fields.push(Field::Pick { arg: name, label, options, selected: usize::from(*on) });
                                    types.push((key.clone(), obj_of("boolean")));
                                }
                                Value::Int(_) | Value::F64(_) if room => {
                                    fields.push(Field::Line { arg: name, label, hint: String::new(), value: line_text(entry) });
                                    types.push((key.clone(), obj_of("number")));
                                }
                                Value::Str(text) if room => {
                                    fields.push(Field::Line { arg: name, label, hint: String::new(), value: text.clone() });
                                    types.push((key.clone(), obj_of("string")));
                                }
                                Value::Arr(items) if room && !items.is_empty() && items.iter().all(|item| matches!(item, Value::Str(_))) => {
                                    let joined = items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
                                    fields.push(Field::Line { arg: name, label, hint: String::new(), value: joined });
                                    types.push((key.clone(), Value::Obj(vec![("type".into(), Value::Str("array".into())), ("items".into(), obj_of("string"))])));
                                }
                                other => inner_fixed.push((key.clone(), other.clone())),
                            }
                        }
                        maps.push((arg.to_string(), inner_fixed));
                        typed_maps.push((arg.to_string(), types));
                    }
                    _ => keep_fixed(&mut fixed),
                }
                continue;
            }
            match shape_of(property) {
                Shape::Pick(values) if fields.len() < MAX_FIELDS => {
                    let mut options: Vec<(String, Option<Value>)> = Vec::new();
                    if !is_required {
                        options.push(("default".to_string(), None));
                    }
                    let is_bool = values.len() == 2 && values[0] == Value::Bool(false);
                    for option in &values {
                        let text = if is_bool {
                            if *option == Value::Bool(true) { "yes".to_string() } else { "no".to_string() }
                        } else {
                            option_label(option)
                        };
                        options.push((text, Some(option.clone())));
                    }
                    let selected = value.and_then(|value| options.iter().position(|(_, option)| option.as_ref() == Some(value))).unwrap_or(0);
                    fields.push(Field::Pick { arg: arg.to_string(), label, options, selected });
                }
                Shape::Line if fields.len() < MAX_FIELDS => {
                    let hint = property.get("description").and_then(Value::as_str).unwrap_or("").to_string();
                    let text = match value {
                        Some(Value::Arr(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                        Some(other) => line_text(other),
                        None => String::new(),
                    };
                    fields.push(Field::Line { arg: arg.to_string(), label, hint, value: text });
                }
                _ => keep_fixed(&mut fixed),
            }
        }
        let title = layout.map(|l| l.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| humanise(tool));
        let confirm = layout.map(|l| l.confirm.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| DEFAULT_CONFIRM.to_string());
        // The keys of an expanded map are typed in the schema the edits are checked against.
        for (arg, types) in typed_maps {
            let Value::Obj(top) = &mut input else { break };
            let Some((_, Value::Obj(properties))) = top.iter_mut().find(|(name, _)| name == "properties") else { break };
            if let Some((_, Value::Obj(map))) = properties.iter_mut().find(|(name, _)| *name == arg) {
                map.retain(|(name, _)| name != "properties");
                map.push(("properties".into(), Value::Obj(types)));
            }
        }
        Ok(Card { tool: tool.to_string(), title, fields, confirm, why: first_sentence(description), fixed, problem: None, maps, schema: input })
    }

    /// The values as prefilled.
    pub fn values(&self) -> CardValues {
        let mut values = CardValues::default();
        for field in &self.fields {
            match field {
                Field::Pick { selected, .. } => values.picks.push(*selected),
                Field::Line { value, .. } => values.lines.push(value.clone()),
            }
        }
        values
    }

    /// The tool's arguments for `values`: picks give their option's value
    /// (`None` leaves the argument out), lines are typed by the schema (an
    /// empty line on an optional argument is left out; a list of strings is
    /// split at commas), and the fixed arguments follow unchanged.
    pub fn args(&self, values: &CardValues) -> Value {
        let required: Vec<&str> = self.schema.get("required").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect();
        let (mut pick, mut line) = (0, 0);
        let mut args: Vec<(String, Value)> = Vec::new();
        // The edited keys of each expanded map, then its kept entries.
        let mut inside: Vec<Vec<(String, Value)>> = vec![Vec::new(); self.maps.len()];
        for field in &self.fields {
            let (map, name) = self.in_map(field.arg());
            let mut put = |args: &mut Vec<(String, Value)>, value: Value| match map {
                Some(at) => inside[at].push((name.to_string(), value)),
                None => args.push((name.to_string(), value)),
            };
            match field {
                Field::Pick { options, selected, .. } => {
                    let index = values.picks.get(pick).copied().unwrap_or(*selected);
                    pick += 1;
                    if let Some((_, Some(value))) = options.get(index) {
                        put(&mut args, value.clone());
                    }
                }
                Field::Line { arg, value, .. } => {
                    let text = values.lines.get(line).cloned().unwrap_or_else(|| value.clone());
                    line += 1;
                    let is_required = required.contains(&arg.as_str());
                    let property = match map {
                        Some(at) => self.schema.get("properties").and_then(|p| p.get(&self.maps[at].0)).and_then(|m| m.get("properties")).and_then(|p| p.get(name)),
                        None => self.schema.get("properties").and_then(|p| p.get(arg)),
                    };
                    let array = property.is_some_and(is_string_array);
                    if text.trim().is_empty() {
                        if array {
                            if is_required {
                                put(&mut args, Value::Arr(Vec::new()));
                            }
                        } else if is_required && property.is_some_and(|p| type_names(p).contains(&"string")) {
                            // A required string stays; the check says it must not be empty.
                            put(&mut args, Value::Str(String::new()));
                        }
                        continue;
                    }
                    if array {
                        let items = text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| Value::Str(s.to_string())).collect();
                        put(&mut args, Value::Arr(items));
                    } else if let Some(property) = property {
                        put(&mut args, typed_line(property, &text));
                    } else {
                        put(&mut args, Value::Str(text));
                    }
                }
            }
        }
        for (at, (arg, kept)) in self.maps.iter().enumerate() {
            let mut entries = std::mem::take(&mut inside[at]);
            entries.extend(kept.iter().cloned());
            if !entries.is_empty() {
                args.push((arg.clone(), Value::Obj(entries)));
            }
        }
        for (arg, value) in &self.fixed {
            if !args.iter().any(|(name, _)| name == arg) {
                args.push((arg.clone(), value.clone()));
            }
        }
        Value::Obj(args)
    }
}

fn type_matches(name: &str, value: &Value) -> bool {
    match (name, value) {
        ("string", Value::Str(_)) | ("boolean", Value::Bool(_)) | ("null", Value::Null) => true,
        ("array", Value::Arr(_)) | ("object", Value::Obj(_)) => true,
        ("integer", Value::Int(_)) | ("number", Value::Int(_) | Value::F64(_)) => true,
        ("integer", Value::F64(n)) => n.fract() == 0.0,
        _ => false,
    }
}

fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::F64(n) => Some(*n),
        _ => None,
    }
}

fn check(path: &str, schema: &Value, value: &Value, why: &mut Vec<String>) {
    let types = type_names(schema);
    if !types.is_empty() && !types.iter().any(|name| type_matches(name, value)) {
        let wants = types.join(" or ");
        why.push(format!("`{path}` must be {wants}"));
        return;
    }
    if let Some(Value::Arr(allowed)) = schema.get("enum") {
        if !allowed.contains(value) {
            let list: Vec<String> = allowed.iter().map(option_label).collect();
            why.push(format!("`{path}` must be one of: {}", list.join(", ")));
            return;
        }
    }
    if let Some(n) = number_of(value) {
        if let Some(min) = schema.get("minimum").and_then(number_of) {
            if n < min {
                why.push(format!("`{path}` must be at least {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(number_of) {
            if n > max {
                why.push(format!("`{path}` must be at most {max}"));
            }
        }
    }
    match value {
        Value::Arr(items) => {
            if let Some(items_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(&format!("{path}[{i}]"), items_schema, item, why);
                }
            }
        }
        Value::Obj(fields) => check_object(path, schema, fields, why),
        _ => {}
    }
}

fn check_object(path: &str, schema: &Value, fields: &[(String, Value)], why: &mut Vec<String>) {
    let properties: &[(String, Value)] = match schema.get("properties") {
        Some(Value::Obj(properties)) => properties,
        _ => &[],
    };
    if let Some(required) = schema.get("required").and_then(Value::as_arr) {
        for name in required.iter().filter_map(Value::as_str) {
            let at = |name: &str| if path.is_empty() { name.to_string() } else { format!("{path}.{name}") };
            match fields.iter().find(|(key, _)| key == name) {
                None => why.push(format!("`{}` is required", at(name))),
                Some((_, Value::Str(text))) if text.trim().is_empty() && properties.iter().any(|(key, p)| key == name && type_names(p) == ["string"]) => {
                    why.push(format!("`{}` must not be empty", at(name)));
                }
                _ => {}
            }
        }
    }
    let closed = matches!(schema.get("additionalProperties"), Some(Value::Bool(false)));
    for (name, value) in fields {
        let at = if path.is_empty() { name.clone() } else { format!("{path}.{name}") };
        match properties.iter().find(|(key, _)| key == name) {
            Some((_, property)) => check(&at, property, value, why),
            None if closed => why.push(format!("`{at}` is not an argument of this tool")),
            None => {}
        }
    }
}

/// Check `args` against a tool's argument schema: required arguments, types,
/// enum members, number bounds, nested lists and objects. `Err` carries
/// every reason, one sentence each.
pub fn validate_args(schema: &Value, args: &Value) -> Result<(), Vec<String>> {
    let Value::Obj(fields) = args else {
        return Err(vec!["the arguments must be a JSON object".to_string()]);
    };
    let mut why = Vec::new();
    check_object("", schema, fields, &mut why);
    if why.is_empty() { Ok(()) } else { Err(why) }
}

impl Card {
    /// The schema edited arguments are checked against (the tool's, with the
    /// keys of an expanded map typed).
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    /// The expanded map `arg` (`params.steps`) belongs to, and the key in it.
    fn in_map<'a>(&self, arg: &'a str) -> (Option<usize>, &'a str) {
        for (at, (map, _)) in self.maps.iter().enumerate() {
            if let Some(key) = arg.strip_prefix(map.as_str()).and_then(|rest| rest.strip_prefix('.')) {
                return (Some(at), key);
            }
        }
        (None, arg)
    }

    /// [`validate_args`] against this card's own schema.
    pub fn validate(&self, args: &Value) -> Result<(), Vec<String>> {
        validate_args(&self.schema, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_strict_json::{obj, parse, s};

    const SCHEMA: &str = r#"{"type":"object","properties":{
        "prompt":{"type":"string","description":"What to make"},
        "seconds":{"type":"integer","minimum":1,"maximum":60},
        "scale":{"type":"number"},
        "shape":{"type":"string","enum":["wide","tall"]},
        "loud":{"type":"boolean"},
        "tags":{"type":"array","items":{"type":"string"}},
        "marks":{"type":"array","items":{"type":"string","enum":["a","b"]}},
        "extra":{"type":"object"},
        "wait":{"type":"boolean"},
        "either":{"anyOf":[{"type":"string"},{"type":"number"}]}
    },"required":["prompt","shape"]}"#;

    fn card(prefill: Value, layout: Option<&CardLayout>) -> Card {
        Card::for_schema("make_clip", "Makes a clip. Takes a while.", SCHEMA, layout, &prefill).unwrap()
    }

    fn field<'a>(card: &'a Card, arg: &str) -> &'a Field {
        card.fields.iter().find(|f| f.arg() == arg).unwrap_or_else(|| panic!("no field {arg}"))
    }

    #[test]
    fn every_type_maps_to_its_field() {
        let card = card(Value::Obj(Vec::new()), None);
        assert!(matches!(field(&card, "prompt"), Field::Line { hint, .. } if hint == "What to make"));
        assert!(matches!(field(&card, "seconds"), Field::Line { .. }));
        assert!(matches!(field(&card, "scale"), Field::Line { .. }));
        assert!(matches!(field(&card, "tags"), Field::Line { .. }));
        assert!(matches!(field(&card, "shape"), Field::Pick { options, .. } if options.len() == 2));
        // An optional boolean: default, no, yes.
        assert!(matches!(field(&card, "loud"), Field::Pick { options, .. }
            if options.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>() == ["default", "no", "yes"]));
        // A free-form map nobody prefilled, lists of enums and anyOf are not editable: fixed.
        for arg in ["marks", "extra", "either"] {
            assert!(card.fields.iter().all(|f| f.arg() != arg), "{arg}");
        }
        assert_eq!(card.title, "Make clip");
        assert_eq!(card.confirm, "Make it");
        assert_eq!(card.why, "Makes a clip.");
        // Required fields come first, in schema order.
        assert_eq!(card.fields[0].arg(), "prompt");
        assert_eq!(card.fields[1].arg(), "shape");
    }

    #[test]
    fn defaults_and_required_handling() {
        let card = card(obj(vec![("prompt", s("a dog"))]), None);
        // A required enum has no default row and starts on its first option.
        assert!(matches!(field(&card, "shape"), Field::Pick { options, selected: 0, .. } if options[0].0 == "wide"));
        // An optional one starts on "default", which leaves the argument out.
        let values = card.values();
        let args = card.args(&values);
        assert_eq!(args.get("prompt"), Some(&s("a dog")));
        assert_eq!(args.get("shape"), Some(&s("wide")));
        assert!(args.get("loud").is_none() && args.get("seconds").is_none() && args.get("tags").is_none());
        assert!(card.validate(&args).is_ok());
        // An empty required string is kept and refused by the check.
        let args = card.args(&CardValues { picks: values.picks.clone(), lines: vec![String::new(); values.lines.len()] });
        let why = card.validate(&args).unwrap_err();
        assert!(why.iter().any(|w| w.contains("`prompt` must not be empty")), "{why:?}");
    }

    #[test]
    fn prefill_selects_options_and_fills_lines() {
        let prefill = parse(br#"{"prompt":"x","shape":"tall","loud":true,"seconds":12,"scale":1.5,"tags":["a","b"],"nope":1,"wait":null}"#).unwrap();
        let card = card(prefill, None);
        assert!(matches!(field(&card, "shape"), Field::Pick { selected: 1, .. }));
        assert!(matches!(field(&card, "loud"), Field::Pick { options, selected, .. } if options[*selected].0 == "yes"));
        assert!(matches!(field(&card, "seconds"), Field::Line { value, .. } if value == "12"));
        assert!(matches!(field(&card, "tags"), Field::Line { value, .. } if value == "a, b"));
        let args = card.args(&card.values());
        assert_eq!(args.get("seconds"), Some(&Value::Int(12)));
        assert_eq!(args.get("scale"), Some(&Value::F64(1.5)));
        assert_eq!(args.get("loud"), Some(&Value::Bool(true)));
        assert_eq!(args.get("tags"), Some(&Value::Arr(vec![s("a"), s("b")])));
        assert!(args.get("nope").is_none() && args.get("wait").is_none());
    }

    #[test]
    fn string_arrays_split_at_commas() {
        let card = card(obj(vec![("prompt", s("p"))]), None);
        let mut values = card.values();
        let at = card.fields.iter().filter(|f| matches!(f, Field::Line { .. })).position(|f| f.arg() == "tags").unwrap();
        values.lines[at] = " one, two ,, three ".into();
        assert_eq!(card.args(&values).get("tags"), Some(&Value::Arr(vec![s("one"), s("two"), s("three")])));
        values.lines[at] = "  ".into();
        assert!(card.args(&values).get("tags").is_none(), "an empty optional list is left out");
    }

    #[test]
    fn fixed_and_hidden_arguments_pass_through() {
        let prefill = parse(br#"{"prompt":"p","extra":{"k":1},"marks":["a"],"either":3,"wait":true}"#).unwrap();
        let layout = CardLayout::new("Make a clip", "Render").hidden("wait");
        let card = card(prefill, Some(&layout));
        assert!(card.fields.iter().all(|f| f.arg() != "wait"));
        assert_eq!(card.fixed.len(), 3);
        let args = card.args(&card.values());
        assert_eq!(args.get("extra"), Some(&obj(vec![("k", Value::Int(1))])));
        assert_eq!(args.get("marks"), Some(&Value::Arr(vec![s("a")])));
        assert_eq!(args.get("either"), Some(&Value::Int(3)));
        assert_eq!(args.get("wait"), Some(&Value::Bool(true)));
        assert_eq!((card.title.as_str(), card.confirm.as_str()), ("Make a clip", "Render"));
    }

    #[test]
    fn a_prefilled_free_form_map_gets_a_field_per_key() {
        let prefill = parse(br#"{"prompt":"p","extra":{"seconds":8,"voice_name":"en","loop":true,"tags":["a","b"],"nested":{"k":1}}}"#).unwrap();
        let card = card(prefill, None);
        let names: Vec<&str> = card.fields.iter().map(Field::arg).filter(|arg| arg.starts_with("extra.")).collect();
        assert_eq!(names, ["extra.seconds", "extra.voice_name", "extra.loop", "extra.tags"]);
        assert!(matches!(field(&card, "extra.seconds"), Field::Line { label, value, .. } if label == "Seconds" && value == "8"));
        assert!(matches!(field(&card, "extra.voice_name"), Field::Line { label, .. } if label == "Voice name"));
        assert!(matches!(field(&card, "extra.loop"), Field::Pick { options, selected: 1, .. } if options[1].0 == "yes"));
        // Untouched, the object comes back as it was (nested kept inside it).
        let args = card.args(&card.values());
        assert_eq!(args.get("extra"), card_prefill_extra(&card_json()).as_ref());
        // Edited: a number stays a number, a yes/no flips, an empty line drops its key.
        let mut values = card.values();
        let line = |arg: &str| card.fields.iter().filter(|f| matches!(f, Field::Line { .. })).position(|f| f.arg() == arg).unwrap();
        values.lines[line("extra.seconds")] = "30".into();
        values.lines[line("extra.voice_name")] = String::new();
        let pick = card.fields.iter().filter(|f| matches!(f, Field::Pick { .. })).position(|f| f.arg() == "extra.loop").unwrap();
        values.picks[pick] = 0;
        let args = card.args(&values);
        let extra = args.get("extra").unwrap();
        assert_eq!(extra.get("seconds"), Some(&Value::Int(30)));
        assert_eq!(extra.get("loop"), Some(&Value::Bool(false)));
        assert!(extra.get("voice_name").is_none());
        assert_eq!(extra.get("nested"), Some(&obj(vec![("k", Value::Int(1))])));
        assert!(card.validate(&args).is_ok());
        // A word in the number field is refused, naming the field.
        values.lines[line("extra.seconds")] = "long".into();
        let why = card.validate(&card.args(&values)).unwrap_err();
        assert!(why.iter().any(|w| w.contains("`extra.seconds` must be number")), "{why:?}");
        // No prefilled map: no field, nothing sent.
        let bare = card_without_extra();
        assert!(bare.fields.iter().all(|f| !f.arg().starts_with("extra")));
        assert!(bare.args(&bare.values()).get("extra").is_none());
    }

    fn card_json() -> Value {
        parse(br#"{"seconds":8,"voice_name":"en","loop":true,"tags":["a","b"],"nested":{"k":1}}"#).unwrap()
    }

    fn card_prefill_extra(extra: &Value) -> Option<Value> {
        Some(extra.clone())
    }

    fn card_without_extra() -> Card {
        card(obj(vec![("prompt", s("p"))]), None)
    }

    #[test]
    fn free_form_map_fields_beyond_the_limit_stay_inside_the_object() {
        let keys: Vec<String> = (0..MAX_FIELDS + 2).map(|i| format!(r#""k{i}":{i}"#)).collect();
        let prefill = parse(format!(r#"{{"prompt":"p","extra":{{{}}}}}"#, keys.join(",")).as_bytes()).unwrap();
        let card = card(prefill, None);
        assert_eq!(card.fields.len(), MAX_FIELDS);
        let args = card.args(&card.values());
        assert!(matches!(args.get("extra"), Some(Value::Obj(entries)) if entries.len() == MAX_FIELDS + 2));
    }

    #[test]
    fn a_layout_orders_and_labels() {
        let layout = CardLayout::new("", "").field("seconds", "Length (seconds)").field("prompt", "What to make").field("missing", "Nothing");
        let card = card(Value::Obj(Vec::new()), Some(&layout));
        assert_eq!(card.fields[0].arg(), "seconds");
        assert_eq!(card.fields[1].arg(), "prompt");
        assert!(matches!(&card.fields[0], Field::Line { label, .. } if label == "Length (seconds)"));
        assert!(card.fields.iter().all(|f| f.arg() != "missing"));
        assert_eq!(card.title, "Make clip", "an empty title takes the default");
    }

    #[test]
    fn the_card_stops_at_the_field_limit() {
        let props: Vec<String> = (0..MAX_FIELDS + 3).map(|i| format!(r#""p{i}":{{"type":"string"}}"#)).collect();
        let schema = format!(r#"{{"type":"object","properties":{{{}}}}}"#, props.join(","));
        let prefill = obj(vec![("p14", s("late"))]);
        let card = Card::for_schema("t", "d", &schema, None, &prefill).unwrap();
        assert_eq!(card.fields.len(), MAX_FIELDS);
        assert_eq!(card.fixed, vec![("p14".to_string(), s("late"))]);
        assert!(Card::for_schema("t", "d", "not json", None, &prefill).is_err());
    }

    #[test]
    fn validation_names_every_reason() {
        let schema = parse(SCHEMA.as_bytes()).unwrap();
        let ok = parse(br#"{"prompt":"p","shape":"wide","seconds":30,"tags":["a"]}"#).unwrap();
        assert!(validate_args(&schema, &ok).is_ok());
        let bad = parse(br#"{"shape":"round","seconds":99,"loud":"yes","tags":[1]}"#).unwrap();
        let why = validate_args(&schema, &bad).unwrap_err().join("; ");
        for part in ["`prompt` is required", "`shape` must be one of: wide, tall", "`seconds` must be at most 60", "`loud` must be boolean", "`tags[0]` must be string"] {
            assert!(why.contains(part), "{part} in {why}");
        }
        assert!(validate_args(&schema, &Value::Null).is_err());
        let closed = parse(br#"{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false}"#).unwrap();
        assert!(validate_args(&closed, &parse(br#"{"b":1}"#).unwrap()).unwrap_err()[0].contains("not an argument"));
    }
}
