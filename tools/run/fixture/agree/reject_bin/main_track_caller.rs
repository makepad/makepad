// rejects: `main` function is not allowed to be `#[track_caller]`
#[track_caller]
fn main() {}
