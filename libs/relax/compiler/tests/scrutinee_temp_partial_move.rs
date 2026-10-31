// A field of a value expression (`mk().0`, `three().1.0`) is a place in ONE temporary of the
// enclosing statement (rustc as_place.rs expr_as_place: the base becomes a temporary, the
// projection a place in it; a match / if-let scrutinee is no terminating scope). What is moved
// out of that place is a move path of the temporary: the temporary's drop at the statement's end
// is elaborated to drop only what remains, fields in declaration order (rustc
// elaborate_drop.rs open_drop_for_adt / drop_ladder). Relax dropped the moved value of
// `match mk().0 { Ok(d) => d, .. }` at the statement's end and again with `d` (found by the
// gpuinit lane: it freed a Wayland connection), and dropped all of `two()` at the end of
// `let a = match two().0 { d => d };`. Expected outputs are rustc's (2021 and 2024 editions).
use std::cell::RefCell;

thread_local! { static LOG: RefCell<Vec<String>> = RefCell::new(Vec::new()); }
fn log(s: String) { LOG.with(|l| l.borrow_mut().push(s)); }
fn take() -> Vec<String> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }
fn v(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }

struct D(&'static str);
impl Drop for D { fn drop(&mut self) { log(format!("drop {}", self.0)); } }
struct W(Result<D, ()>);
fn mk() -> W { W(Ok(D("x"))) }

struct Two(D, D);
fn two() -> Two { Two(D("a"), D("b")) }

struct Three(D, Two, D);
fn three() -> Three { Three(D("p"), two(), D("q")) }

struct P(Two, D);
fn mkp() -> P { P(two(), D("c")) }

#[test]
fn match_scrutinee_field_moved_out_of_temporary_drops_once() {
    {
        let d = match mk().0 { Ok(d) => d, Err(_) => panic!() };
        log(format!("got {}", d.0));
    }
    assert_eq!(take(), v(&["got x", "drop x"]));
}

#[test]
fn match_scrutinee_partial_move_drops_remaining_field_at_statement_end() {
    {
        let a = match two().0 { d => d };
        log(format!("got {}", a.0));
    }
    // the temporary's unmoved field `b` drops at the end of the `let` statement, `a` at scope end
    assert_eq!(take(), v(&["drop b", "got a", "drop a"]));
}

#[test]
fn if_let_scrutinee_field_partial_move() {
    {
        if let Two(a, _) = mkp().0 {
            log(format!("got {}", a.0));
        }
        log("after".to_string());
    }
    assert_eq!(take(), v(&["got a", "drop a", "drop b", "drop c", "after"]));
}

#[test]
fn if_let_scrutinee_enum_field_moved_drops_once() {
    {
        if let Ok(d) = mk().0 {
            log(format!("got {}", d.0));
        }
        log("after".to_string());
    }
    assert_eq!(take(), v(&["got x", "drop x", "after"]));
}

#[test]
fn let_field_moved_out_of_temporary() {
    {
        let a = two().0;
        log(format!("got {}", a.0));
    }
    assert_eq!(take(), v(&["drop b", "got a", "drop a"]));
}

#[test]
fn let_nested_field_moved_out_of_temporary() {
    {
        let a = three().1.0;
        log(format!("got {}", a.0));
    }
    // one temporary, `three()`: its remaining parts in declaration order
    assert_eq!(take(), v(&["drop p", "drop b", "drop q", "got a", "drop a"]));
}

#[test]
fn match_nested_field_scrutinee_partial_move() {
    {
        let a = match three().1.0 { d => d };
        log(format!("got {}", a.0));
    }
    assert_eq!(take(), v(&["drop p", "drop b", "drop q", "got a", "drop a"]));
}

#[test]
fn match_nested_enum_field_scrutinee_moves_inner_part() {
    {
        let n = match (mk(), D("y")).0.0 { Ok(d) => d, Err(_) => panic!() };
        log(format!("got {}", n.0));
    }
    assert_eq!(take(), v(&["drop y", "got x", "drop x"]));
}

#[test]
fn match_scrutinee_field_bound_by_ref_drops_whole_temporary() {
    {
        match two().0 {
            ref d => log(format!("in {}", d.0)),
        }
        log("after".to_string());
        let n = match mk().0 { Ok(ref d) => d.0, Err(_) => "none" };
        log(format!("got {}", n));
    }
    assert_eq!(take(), v(&["in a", "drop a", "drop b", "after", "drop x", "got x"]));
}

#[test]
fn match_scrutinee_field_arm_that_does_not_move() {
    {
        let n = match mk().0 { Err(_) => 0, _ => 1 };
        log(format!("got {}", n));
    }
    assert_eq!(take(), v(&["drop x", "got 1"]));
}
