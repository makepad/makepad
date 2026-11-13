// A method call's user type annotation (rustc method/confirm.rs, instantiate_method_args) holds
// the method's own arguments as the call writes them: the turbofish, else fresh variables. What
// method probing inferred for them is no annotation. `[&a[..], &[b]].concat()` infers
// `Item = &'static str` from `a`'s literals first; as an annotation that would make `b` outlive
// `'static` (E0521).

fn total(words: &[&str]) -> usize {
    words.iter().map(|w| w.len()).sum()
}

fn joined_in_closure(extra: &str) -> usize {
    let fixed = ["red", "green"];
    let join = |x: &str| total(&[&fixed[..], &[x]].concat());
    join(extra)
}

fn joined_from_local(extra: &str) -> Vec<&str> {
    let fixed = ["blue"];
    let parts = [&fixed[..], &[extra]];
    parts.concat()
}

fn joined_from_slice(extra: &str) -> usize {
    let fixed = ["one", "two"];
    let parts = [&fixed[..], &[extra], &fixed[1..]];
    parts[..].concat().len()
}

struct Keep;
impl Keep {
    fn pass<T>(&self, t: T) -> T {
        t
    }
    fn first<const N: usize, T: Copy>(&self, a: [T; N]) -> T {
        a[0]
    }
}

trait Echo {
    fn echo<T>(&self, t: T) -> T;
}
impl Echo for Keep {
    fn echo<T>(&self, t: T) -> T {
        t
    }
}

fn written(extra: &str) -> (usize, usize, &str) {
    let w = Keep.pass::<&str>(extra);
    let f = Keep.first::<2, &str>([extra, "x"]);
    let e = Keep.echo::<&str>(extra);
    (w.len(), f.len(), e)
}

#[test]
fn own_method_args_are_not_annotated_from_inference() {
    let owned = String::from("abcd");
    assert_eq!(joined_in_closure(&owned), 12, "closure concat");
    assert_eq!(joined_from_local(&owned), ["blue", "abcd"], "local array concat");
    assert_eq!(joined_from_slice(&owned), 4, "slice concat");
    assert_eq!(written(&owned), (4, 4, "abcd"), "written turbofish");
}
