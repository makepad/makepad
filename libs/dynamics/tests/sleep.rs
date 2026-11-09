use makepad_dynamics::sleep::{sleep_cycle, wake_island};

#[test]
fn sleep_cycles_and_wake_islands() {
    // Hand trace: 0 -> 2 -> 1 -> 0; 3 -> 3. The first cycle's minimum is 0.
    let mut tree_asleep = [2, 0, 1, 3];
    for i in 0..3 {
        assert_eq!(
            sleep_cycle(&tree_asleep, 4, i),
            0,
            "three-tree cycle at {i}"
        );
    }
    assert_eq!(sleep_cycle(&tree_asleep, 4, 3), 3, "singleton cycle");
    assert_eq!(
        tree_asleep,
        [2, 0, 1, 3],
        "cycle lookup does not modify links"
    );

    // Starting at 1 writes -11 in order 1, 0, 2, then stops on returning to 1.
    assert_eq!(
        wake_island(&mut tree_asleep, 4, 1, -11, Some("test"), 0.0),
        3,
        "wake entire cycle"
    );
    assert_eq!(tree_asleep, [-11, -11, -11, 3], "singleton remains asleep");
    assert_eq!(
        sleep_cycle(&tree_asleep, 4, 1),
        -1,
        "awake tree has no cycle"
    );

    // Already awake: min(-5, -11) keeps -11; min(-12, -11) resets just tree 0.
    assert_eq!(
        wake_island(&mut tree_asleep, 4, 0, -5, None, 0.0),
        0,
        "already awake count"
    );
    assert_eq!(
        tree_asleep,
        [-11, -11, -11, 3],
        "do not shorten awake countdown"
    );
    assert_eq!(
        wake_island(&mut tree_asleep, 4, 0, -12, None, 0.0),
        0,
        "reset awake countdown"
    );
    assert_eq!(
        tree_asleep,
        [-12, -11, -11, 3],
        "former cycle no longer linked"
    );

    assert_eq!(
        wake_island(&mut tree_asleep, 4, 3, -11, None, 0.0),
        1,
        "wake singleton"
    );
    assert_eq!(tree_asleep, [-12, -11, -11, -11], "all trees awake");
}
