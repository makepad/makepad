// std-os lane: std::time Instant / SystemTime (differential: os_diff.sh)
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn a01_instant() {
    let a = Instant::now();
    let b = Instant::now();
    println!("| {} {}", b >= a, (b - a) < Duration::from_secs(1));
    println!("| {:?} {:?}", a.checked_duration_since(b + Duration::from_secs(1)), (a + Duration::from_millis(5)).duration_since(a));
    println!("| {:?}", a.saturating_duration_since(a + Duration::from_secs(3)));
    println!("| {} {}", a.checked_add(Duration::MAX).is_none(), a.checked_sub(Duration::from_secs(1)).is_some());
    let mut c = a;
    c += Duration::from_millis(1500);
    c -= Duration::from_millis(500);
    println!("| {:?} {}", c - a, a.elapsed() < Duration::from_secs(5));
}

#[test]
fn a02_system_time() {
    let now = SystemTime::now();
    let since = now.duration_since(UNIX_EPOCH).unwrap();
    println!("| {}", since.as_secs() > 1_700_000_000);
    let e = UNIX_EPOCH.duration_since(now).unwrap_err();
    println!("| {} {}", e, e.duration() == since);
    let t = UNIX_EPOCH + Duration::new(1234, 5678);
    println!("| {:?} {:?}", t.duration_since(UNIX_EPOCH), t);
    println!("| {:?} {:?}", UNIX_EPOCH, SystemTime::UNIX_EPOCH == UNIX_EPOCH);
    println!("| {:?}", (t - Duration::from_secs(2000)).duration_since(UNIX_EPOCH));
    println!("| {:?}", t.checked_sub(Duration::new(1234, 5679)).map(|x| x < UNIX_EPOCH));
    println!("| {}", now.elapsed().unwrap() < Duration::from_secs(5));
}
