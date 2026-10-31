struct D { node: String, cutoff: String, res: String }
impl D {
    fn path(&self, field: &str) -> String {
        let given = match field {
            "cutoff" => &self.cutoff,
            "res" => &self.res,
            _ => "",
        };
        if given.is_empty() { format!("{}.{field}", self.node) } else { given.to_string() }
    }
    fn first(&self, b: bool) -> usize {
        let s = if b { &self.node } else { "x" };
        s.len()
    }
}
#[test]
fn match_arms_deref_coerce_to_later_str() {
    let d = D { node: "n".into(), cutoff: "c".into(), res: String::new() };
    assert_eq!(d.path("cutoff"), "c");
    assert_eq!(d.path("res"), "n.res");
    assert_eq!(d.path("zz"), "n.zz");
}

#[test]
fn if_arms_deref_coerce_to_later_str() {
    let d = D { node: "node".into(), cutoff: String::new(), res: String::new() };
    assert_eq!(d.first(true), 4);
    assert_eq!(d.first(false), 1);
}

#[test]
fn match_arms_copy_all_deref_steps() {
    let first = Box::new(String::from("first"));
    let second = Box::new(String::from("second"));
    let first_ref = &first;
    let second_ref = &second;
    for field in 0..3 {
        // A built-in reference deref followed by Box::deref and String::deref.
        let given = match field {
            0 => &first_ref,
            1 => &second_ref,
            _ => "fallback",
        };
        assert_eq!(given, match field { 0 => "first", 1 => "second", _ => "fallback" });
    }
}
