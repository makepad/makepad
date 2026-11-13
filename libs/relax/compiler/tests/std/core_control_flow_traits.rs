// core's `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]` on ops::ControlFlow, and its value
// accessors (stable since 1.83): the std here declared the bare enum with is_break, is_continue
// and break_value, so a ControlFlow could not be printed, compared, copied or used as a key.
// Found by the try_rfold test next to this file, which compares the ControlFlow a fold returns.
use std::collections::HashSet;
use std::ops::ControlFlow;

fn first_negative(v: &[i32]) -> ControlFlow<i32, usize> {
    v.iter().try_fold(0usize, |seen, &x| if x < 0 { ControlFlow::Break(x) } else { ControlFlow::Continue(seen + 1) })
}

#[test]
fn control_flow_derives() {
    let hit = first_negative(&[4, 7, -2, 9]);
    let clean = first_negative(&[4, 7]);
    assert_eq!(hit, ControlFlow::Break(-2));
    assert_eq!(clean, ControlFlow::Continue(2));
    assert_ne!(hit, clean);
    assert_eq!(format!("{:?} {:?}", hit, clean), "Break(-2) Continue(2)");
    assert_eq!(format!("{:#?}", ControlFlow::<&str, ()>::Break("stop")), "Break(\n    \"stop\",\n)");
    assert_eq!(format!("{:?}", ControlFlow::<u8>::Continue(())), "Continue(())");

    // Copy when both sides are, Clone otherwise
    let copy = hit;
    assert_eq!(copy, hit);
    let owned: ControlFlow<String, Vec<u8>> = ControlFlow::Break("why".to_string());
    let cloned = owned.clone();
    assert_eq!(cloned, owned);
    assert_ne!(cloned, ControlFlow::Continue(vec![]));

    let mut set = HashSet::new();
    assert!(set.insert(hit));
    assert!(set.insert(clean));
    assert!(!set.insert(ControlFlow::Break(-2)));
    assert!(set.insert(ControlFlow::Continue(3)));
    assert!(set.contains(&ControlFlow::Continue(2)) && !set.contains(&ControlFlow::Break(2)));
    assert_eq!(set.len(), 3);
}

#[test]
fn control_flow_values() {
    let hit = first_negative(&[1, -5]);
    let clean = first_negative(&[1, 2, 3]);
    assert_eq!((hit.is_break(), hit.is_continue()), (true, false));
    assert_eq!((hit.break_value(), hit.continue_value()), (Some(-5), None));
    assert_eq!((clean.break_value(), clean.continue_value()), (None, Some(3)));
    assert_eq!(hit.map_break(|b| b.to_string()), ControlFlow::Break("-5".to_string()));
    assert_eq!(clean.map_break(|b| b.to_string()), ControlFlow::Continue(3));
    assert_eq!(clean.map_continue(|c| c * 2), ControlFlow::Continue(6));
    assert_eq!(hit.map_continue(|c| c * 2), ControlFlow::Break(-5));
    let mut calls = 0;
    let kept = hit.map_continue(|c| {
        calls += 1;
        c
    });
    assert_eq!((kept, calls), (hit, 0));
}
