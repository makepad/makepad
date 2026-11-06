// M10: a fieldless enum read as a number (`as`) by running code.
//  v1: a variant added after the running ones keeps their numbers: applies in place.
//  v2: a variant inserted before a running one moves its number, which running code may have
//      kept: the restart rung.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Level {
    Low,
    High,
    Max,
}

fn index(l: Level) -> usize {
    l as usize
}

#[test]
fn run() {
    println!("out index {} {} {:?}", index(Level::High), index(Level::Max), Level::Low);
}
