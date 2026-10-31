// rustc_expand mbe/quoted.rs: parse_tree retains body DocComment tokens;
// mbe/macro_rules.rs: parser_from_cx desugars them before the next match.
macro_rules! service {
    ($name:ident, GET) => {
        service!(
            /// Route GET requests.
            /// Including "# in the documentation.
            $name, GET
        );
    };
    ($name:ident, CONNECT) => {
        service!(
            /** Route CONNECT requests. */
            $name, CONNECT
        );
    };
    ($name:ident, $method:ident) => {
        service!(
            #[doc = concat!("Route ", stringify!($method))]
            /// Another method.
            $name, $method
        );
    };
    ($(#[$m:meta])+ $name:ident, $method:ident) => {
        $(#[$m])+
        fn $name() -> usize { [$(stringify!($m)),+].len() }
    };
}

service!(get, GET);
service!(connect, CONNECT);
service!(post, POST);

// rustc_expand mbe/macro_parser.rs: TtParser::match_one ignores matcher docs.
macro_rules! matcher_doc {
    (
        /// Ignored in the matcher.
        $value:expr
    ) => { $value };
}

macro_rules! define_matcher {
    () => {
        macro_rules! generated_matcher {
            (
                /// Still ignored after transcription.
            ) => { 43 };
        }
    };
}
define_matcher!();

#[test]
fn body_docs_select_the_terminating_arm() {
    assert_eq!(get(), 2);
    assert_eq!(connect(), 1);
    assert_eq!(post(), 2);
    assert_eq!(matcher_doc!(42), 42);
    assert_eq!(generated_matcher!(), 43);
}
