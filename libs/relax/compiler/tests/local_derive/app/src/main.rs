// A proc-macro derive on items declared inside fn bodies: the generated impls are visible to
// method resolution like those of module items (rustc: block items live in an anonymous module;
// impls are global). Full-check: libs/ai/hub world_backend.rs `#[derive(SerJson)] struct JobLine`.
use local_derive_mac::Named;

pub trait Named {
    fn name(&self) -> &'static str;
}

#[derive(Named)]
struct Outer;

fn local_struct() -> &'static str {
    #[derive(Named, Clone)]
    struct JobLine {
        _seed: u64,
    }
    let job = JobLine { _seed: 7 };
    job.clone().name()
}

fn local_enum_in_closure() -> &'static str {
    #[derive(Named)]
    enum Mode {
        _A,
        B,
    }
    let f = || Mode::B.name();
    f()
}

fn nested_block() -> usize {
    let n = {
        #[derive(Named)]
        struct Inner(u8);
        Inner(1).name().len()
    };
    n
}

fn main() {
    assert_eq!(Outer.name(), "Outer");
    assert_eq!(local_struct(), "JobLine");
    assert_eq!(local_enum_in_closure(), "Mode");
    assert_eq!(nested_block(), 5);
    println!("local derives ok");
}
