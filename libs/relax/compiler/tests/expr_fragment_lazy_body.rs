// rustc --edition=2021 --test expr_fragment_lazy_body.rs
// makepad test expr_fragment_lazy_body.rs
// An `$e:expr` fragment of several tokens is transcribed in invisible parentheses. A fn body in
// macro output that holds one is parsed with its expansion: the later parse of the bodies a
// load skips (`makepad run`) has no record of those parentheses, so there a false `#[cfg]` on the
// fragment left `()` in the list (ruma-common's `metadata!`, `required_client_scopes`) and
// `stringify!` printed them.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Scope { Full, Admin }

pub trait Metadata {
    fn scopes() -> &'static [Scope];
}

// ruma-common 0.20 `metadata!`: the list arrives as a `tt`, is matched as `$scope:expr`
// repetitions and transcribed into `&[$($scope,)+]` inside a trait method.
macro_rules! metadata {
    ( @for $request_type:ty, $( $field:ident: $rhs:tt ),+ $(,)? ) => {
        impl Metadata for $request_type {
            $( metadata!(@field $field: $rhs); )+
        }
    };
    ( $( $field:ident: $rhs:tt ),+ $(,)? ) => {
        metadata!{ @for Request, $( $field: $rhs),+ }
    };
    ( @field scopes: [$( $scope:expr ),+ $(,)?] ) => {
        fn scopes() -> &'static [Scope] {
            &[$($scope,)+]
        }
    };
}
pub struct Request;
metadata! {
    scopes: [
        #[cfg(not(any()))]
        Scope::Full,
        #[cfg(any())]
        Scope::Admin,
    ],
}

macro_rules! named {
    ($name:ident, $e:expr) => {
        fn $name() -> (&'static str, i32) {
            (stringify!($e), $e)
        }
    };
}
named!(sum, 1 + 2);

macro_rules! tuple_fn {
    ($name:ident, $( $e:expr ),+) => {
        fn $name() -> (u8, u8) {
            ($($e,)+)
        }
    };
}
tuple_fn!(pair, 1 + 1, #[cfg(any())] 9, #[cfg(all())] 3);

fn checks() {
    assert_eq!(Request::scopes(), &[Scope::Full]);
    assert_eq!(sum(), ("1 + 2", 3));
    assert_eq!(pair(), (2, 3));
}

#[test]
fn fragments_in_macro_made_fn_bodies() {
    checks();
}

// `makepad test` loads a single file with every body parsed. `makepad run` skips the bodies a
// later parse can do alone: run the same items as a package in a fresh process.
#[cfg(relax)]
#[test]
fn the_same_bodies_under_makepad_run() {
    let dir = std::env::temp_dir().join(format!("relax-expr-fragment-lazy-body-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"expr_fragment_lazy_body\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[[bin]]\nname=\"expr_fragment_lazy_body\"\npath=\"main.rs\"\n[workspace]\n").unwrap();
    let source = include_str!("expr_fragment_lazy_body.rs").to_string() + "\nfn main() { checks(); println!(\"lazy bodies ok\"); }\n";
    std::fs::write(dir.join("main.rs"), source).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["run", "expr_fragment_lazy_body"])
        .current_dir(&dir)
        .env("MAKEPAD_RUN_HOME", dir.join("home"))
        .env("MAKEPAD_RUN_LAZY_BODIES", "1")
        .output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("lazy bodies ok"), "{stdout}\n{stderr}");
}
