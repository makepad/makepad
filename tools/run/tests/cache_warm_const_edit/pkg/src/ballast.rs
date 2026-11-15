// Definitions the fill run's crate has: run.py's fewer_definitions starts a session without this
// module (and its call), so the cache's image holds 300 more definitions than that program.
macro_rules! ten {
    ($($t:tt)*) => { $($t)* $($t)* $($t)* $($t)* $($t)* $($t)* $($t)* $($t)* $($t)* $($t)* };
}
ten! { ten! { const _: u64 = 1; } }
ten! { ten! { const _: u64 = 2; } }
ten! { ten! { const _: u64 = 3; } }

pub fn ballast() -> u64 {
    1
}
