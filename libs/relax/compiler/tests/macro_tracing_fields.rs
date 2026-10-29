// tracing 0.1.44's relevant arms, in source order, with an owned fake callsite.
// rustc_expand/mbe/macro_rules.rs: try_match_macro takes the first matching arm;
// mbe/macro_parser.rs: TtParser::match_one matches sigils and separators literally.
extern crate self as tracing;

pub struct Level;
impl Level {
    pub const TRACE: u8 = 0;
    pub const INFO: u8 = 1;
    pub const WARN: u8 = 2;
    pub const ERROR: u8 = 3;
}

struct Callsite {
    name: &'static str,
    target: &'static str,
    level: u8,
    fields: Vec<String>,
    values: Vec<String>,
}
impl Callsite {
    fn new(name: &'static str, target: &'static str, level: u8, fields: &[&str]) -> Self {
        Self {
            name, target, level,
            fields: fields.iter().map(|name| name.replace(' ', "")).collect(),
            values: Vec::new(),
        }
    }
    fn value_set_all(mut self, values: Vec<String>) -> Self {
        assert_eq!(self.fields.len(), values.len());
        self.values = values;
        self
    }
}

fn render(args: std::fmt::Arguments<'_>) -> String { std::fmt::format(args) }

#[macro_export]
macro_rules! span {
    (target: $target:expr, parent: $parent:expr, $lvl:expr, $name:expr, $($fields:tt)*) => {{
        let _ = $parent;
        $crate::valueset!(
            $crate::Callsite::new($name, $target, $lvl, $crate::fieldset!($($fields)*)),
            $($fields)*
        )
    }};
    (target: $target:expr, $lvl:expr, $name:expr, $($fields:tt)*) => {
        $crate::valueset!(
            $crate::Callsite::new($name, $target, $lvl, $crate::fieldset!($($fields)*)),
            $($fields)*
        )
    };
    (target: $target:expr, $lvl:expr, $name:expr) => {
        $crate::span!(target: $target, $lvl, $name,)
    };
    ($lvl:expr, $name:expr, $($fields:tt)*) => {
        $crate::span!(target: module_path!(), $lvl, $name, $($fields)*)
    };
    ($lvl:expr, $name:expr) => {
        $crate::span!(target: module_path!(), $lvl, $name,)
    };
}

#[macro_export]
macro_rules! event {
    (target: $target:expr, $lvl:expr, { $($fields:tt)* }) => {
        $crate::valueset!(
            $crate::Callsite::new("event", $target, $lvl, $crate::fieldset!($($fields)*)),
            $($fields)*
        )
    };
    (target: $target:expr, $lvl:expr, { $($fields:tt)* }, $($arg:tt)+) => {
        $crate::event!(target: $target, $lvl, { message = format_args!($($arg)+), $($fields)* })
    };
    (target: $target:expr, $lvl:expr, $($k:ident).+ = $($fields:tt)*) => {
        $crate::event!(target: $target, $lvl, { $($k).+ = $($fields)* })
    };
    (target: $target:expr, $lvl:expr, $($arg:tt)+) => {
        $crate::event!(target: $target, $lvl, { $($arg)+ })
    };
    ($lvl:expr, { $($fields:tt)* }, $($arg:tt)+) => {
        $crate::event!(target: module_path!(), $lvl, { message = format_args!($($arg)+), $($fields)* })
    };
    ($lvl:expr, $($k:ident).+ = $($field:tt)*) => {
        $crate::event!(target: module_path!(), $lvl, { $($k).+ = $($field)* })
    };
    ($lvl:expr, ?$($k:ident).+, $($field:tt)*) => {
        $crate::event!(target: module_path!(), $lvl, { ?$($k).+, $($field)* })
    };
    ($lvl:expr, %$($k:ident).+, $($field:tt)*) => {
        $crate::event!(target: module_path!(), $lvl, { %$($k).+, $($field)* })
    };
    ($lvl:expr, $($arg:tt)+) => {
        $crate::event!(target: module_path!(), $lvl, { $($arg)+ })
    };
}

#[macro_export]
macro_rules! trace {
    ($($k:ident).+ = $($field:tt)*) => {
        $crate::event!(target: module_path!(), $crate::Level::TRACE, { $($k).+ = $($field)* })
    };
    (?$($k:ident).+, $($field:tt)*) => {
        $crate::event!(target: module_path!(), $crate::Level::TRACE, { ?$($k).+, $($field)* })
    };
    (%$($k:ident).+, $($field:tt)*) => {
        $crate::event!(target: module_path!(), $crate::Level::TRACE, { %$($k).+, $($field)* })
    };
    ($($arg:tt)+) => {
        $crate::event!(target: module_path!(), $crate::Level::TRACE, $($arg)+)
    };
}

macro_rules! warn {
    ($($arg:tt)+) => { $crate::event!(target: module_path!(), $crate::Level::WARN, $($arg)+) };
}
macro_rules! info {
    ($($arg:tt)+) => { $crate::event!(target: module_path!(), $crate::Level::INFO, $($arg)+) };
}
macro_rules! error {
    (?$($k:ident).+, $($field:tt)*) => {
        $crate::event!(target: module_path!(), $crate::Level::ERROR, { ?$($k).+, $($field)* })
    };
    ($($arg:tt)+) => { $crate::event!(target: module_path!(), $crate::Level::ERROR, $($arg)+) };
}

// The valueset_all! arms used by event!/span!, named valueset! here.
// Own each rendered value so no temporary fmt::Arguments escapes its statement.
#[macro_export]
macro_rules! valueset {
    (@ { $(,)* $($val:expr),* $(,)* } $(,)*) => { vec![$($val),*] };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{:?}", &$val)) }, $($rest)*)
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{}", &$val)) }, $($rest)*)
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{}", &$val)) }, $($rest)*)
    };
    (@ { $(,)* $($out:expr),* }, ?$($k:ident).+, $($rest:tt)*) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{:?}", &$($k).+)) }, $($rest)*)
    };
    (@ { $(,)* $($out:expr),* }, %$($k:ident).+, $($rest:tt)*) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{}", &$($k).+)) }, $($rest)*)
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = ?$val:expr) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{:?}", &$val)) },)
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = %$val:expr) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{}", &$val)) },)
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = $val:expr) => {
        $crate::valueset!(@ { $($out),*, $crate::render(format_args!("{}", &$val)) },)
    };
    (@ { $(,)* $($out:expr),* }, $($rest:tt)+) => {
        $crate::valueset!(@ { $crate::render(format_args!($($rest)+)), $($out),* },)
    };
    ($fields:expr, $($kvs:tt)+) => {
        $fields.value_set_all($crate::valueset!(@ { }, $($kvs)+))
    };
    ($fields:expr,) => { $fields.value_set_all(Vec::new()) };
}

#[macro_export]
macro_rules! fieldset {
    (@ { $(,)* $($out:expr),* $(,)* } $(,)*) => { &[$($out),*] };
    (@ { $(,)* $($out:expr),* } $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::fieldset!(@ { $($out),*, stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::fieldset!(@ { $($out),*, stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::fieldset!(@ { $($out),*, stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } ?$($k:ident).+, $($rest:tt)*) => {
        $crate::fieldset!(@ { $($out),*, stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } %$($k:ident).+, $($rest:tt)*) => {
        $crate::fieldset!(@ { $($out),*, stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $($rest:tt)+) => {
        $crate::fieldset!(@ { "message", $($out),*, })
    };
    ($($args:tt)*) => { $crate::fieldset!(@ { } $($args)*,) };
}

#[test]
fn messages_and_line_continuations() {
    let site = trace!("spawned the sending task");
    assert_eq!(site.fields, ["message"]);
    assert_eq!(site.values, ["spawned the sending task"]);
    assert_eq!(site.level, Level::TRACE);
    let err = "offline";
    let site = warn!("errors when applying dependent requests: {err}");
    assert_eq!(site.fields, ["message"]);
    assert_eq!(site.values, ["errors when applying dependent requests: offline"]);
    assert_eq!(site.level, Level::WARN);
    let site = info!("Cannot insert the event because \
                     either the room is gone or the cache is unavailable");
    assert_eq!(site.fields, ["message"]);
    assert_eq!(site.values, ["Cannot insert the event because either the room is gone or the cache is unavailable"]);
    assert_eq!(site.level, Level::INFO);
}

#[test]
fn fields_are_removed_before_formatting_the_message() {
    let txn_id = "txn-7";
    let site = trace!(txn_id = %txn_id, "received a request to send!");
    assert_eq!(site.fields, ["message", "txn_id"]);
    assert_eq!(site.values, ["received a request to send!", "txn-7"]);
    let err = "bad event";
    let site = error!(?err, "Failed to build the (sync) event before the saving in the Event Cache");
    assert_eq!(site.fields, ["message", "err"]);
    assert_eq!(site.values, ["Failed to build the (sync) event before the saving in the Event Cache", "\"bad event\""]);
    assert_eq!(site.level, Level::ERROR);
    let site = trace!(txn_id = %txn_id, error = ?err, "failed: {err}");
    assert_eq!(site.fields, ["message", "txn_id", "error"]);
    assert_eq!(site.values, ["failed: bad event", "txn-7", "\"bad event\""]);
}

struct Room { id: u32 }
impl Room { fn room_id(&self) -> u32 { self.id } }

#[test]
fn dotted_fields_and_instrument_span() {
    let room = Room { id: 42 };
    let site = trace!(%room.id, "room {id}", id = room.id);
    assert_eq!(site.fields, ["message", "room.id"]);
    assert_eq!(site.values, ["room 42", "42"]);
    let site = event!(Level::INFO, ?room.id, "debug room");
    assert_eq!(site.fields, ["message", "room.id"]);
    assert_eq!(site.values, ["debug room", "42"]);
    let site = ::tracing::span!(target: module_path!(), ::tracing::Level::INFO, "sending_task", room_id = %room.room_id());
    assert_eq!(site.name, "sending_task");
    assert_eq!(site.target, module_path!());
    assert_eq!(site.level, Level::INFO);
    assert_eq!(site.fields, ["room_id"]);
    assert_eq!(site.values, ["42"]);
    let site = event!(Level::INFO, { room.id = %room.room_id() }, "room {id}", id = room.id);
    assert_eq!(site.fields, ["message", "room.id"]);
    assert_eq!(site.values, ["room 42", "42"]);
}

// sending_task also has JSON format! calls with `type = ...` after other names.
// rustc_builtin_macros/src/format.rs: parse_args accepts Token::ident() before `=`;
// rustc_ast/src/token.rs: Token::ident includes keyword and interpolated ident tokens.
#[test]
fn keyword_named_format_arguments() {
    let json = format!(
        "{{\
            \"event_id\":\"{event_id}\",\
            \"type\":\"{type}\",\
            \"content\":{content}\
        }}",
        event_id = "$event",
        type = "m.room.message",
        content = "{}",
    );
    assert_eq!(json, "{\"event_id\":\"$event\",\"type\":\"m.room.message\",\"content\":{}}");
    assert_eq!(render(format_args!("{type}", type = 7)), "7");
    assert_eq!(format!("{first}:{type}:{async}:{true}:{self}:{match}", first = 1, type = 2, async = 3, true = 4, self = 5, match = 6), "1:2:3:4:5:6");
    assert_eq!(format!("{type}:{match}", r#type = 2, r#match = 6), "2:6");
    let site = trace!(txn_id = %7, "{kind}:{type}", kind = "event", type = "message");
    assert_eq!(site.fields, ["message", "txn_id"]);
    assert_eq!(site.values, ["event:message", "7"]);
}

macro_rules! named_format {
    ($fmt:literal, $name:ident = $value:expr) => { format!($fmt, $name = $value) };
}

#[test]
fn forwarded_keyword_argument() {
    assert_eq!(named_format!("{type}", type = 9), "9");
}
