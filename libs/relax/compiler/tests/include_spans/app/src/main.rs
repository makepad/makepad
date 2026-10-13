use include_spans_mac::{authored_file, call_location, forward, spans};

include!("included/body.rs");

macro_rules! with_prelude {
    ($body:tt) => {
        spans! {
            use mod.prelude.widgets.*
            $body
        }
    };
}

macro_rules! located {
    () => { (call_location!(), line!(), column!()) };
}

fn main() {
    print!("{}{}{}{}{}{}{}", DIRECT, FORWARDED, NESTED, RULES, EXPRESSION, ATTRIBUTE, DERIVED);
    println!("{} {:?} {:?}", included_file(), CALL, LOCATION);
    let _ = Derived;
    assert_eq!(TEXT, "nested include text\n");
    assert_eq!(included_file().rsplit('/').next().unwrap(), "body.rs");
    print!("{}", with_prelude!({padding + 12}));
    println!("{:?}", located!());
}
