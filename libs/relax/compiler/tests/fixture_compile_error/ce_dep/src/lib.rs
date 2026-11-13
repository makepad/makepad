// rustc refuses this (E0308); a run finds it when main calls it
pub fn broken() -> u32 {
    let n: u32 = "not a number";
    n
}
