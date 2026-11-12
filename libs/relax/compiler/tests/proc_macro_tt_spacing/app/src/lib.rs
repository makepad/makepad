//! A token passed on through macro_rules `tt` captures keeps the spacing it was lexed with (relax
//! issue 9): a proc macro reads `::<`, `!-` and `&&!` as given directly. A token written in a
//! macro's body reaches it `Alone` (rustc transcribes body tokens so).

macro_rules! fwd {
    ($($tokens:tt)*) => { spacing_macro::spacings!($($tokens)*) };
}

macro_rules! fwd_again {
    ($($tokens:tt)*) => { fwd!($($tokens)*) };
}

macro_rules! written {
    () => { spacing_macro::spacings!(f::<u32>() !-x &&!x) };
}

#[cfg(test)]
const RUSTC: &str = "':' Joint, ':' Joint, '<' Alone, '>' Alone, '!' Joint, '-' Alone, '&' Joint, '&' Joint, '!' Alone";
#[cfg(test)]
const BODY: &str = "':' Joint, ':' Alone, '<' Alone, '>' Alone, '!' Alone, '-' Alone, '&' Joint, '&' Alone, '!' Alone";

#[test]
fn a_proc_macro_reads_the_spacing_rustc_gives() {
    assert_eq!(spacing_macro::spacings!(f::<u32>() !-x &&!x), RUSTC, "given directly");
    assert_eq!(fwd!(f::<u32>() !-x &&!x), RUSTC, "through a tt repetition");
    assert_eq!(fwd_again!(f::<u32>() !-x &&!x), RUSTC, "through two");
    assert_eq!(written!(), BODY, "written in a definition's body");
    assert_eq!(forward::forward!(f::<u32>() !-x &&!x), RUSTC, "through another crate's macro");
    assert_eq!(forward::written!(), BODY, "written in another crate's macro");
}
