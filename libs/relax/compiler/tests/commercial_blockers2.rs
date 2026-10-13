//! Commercial-lane blockers, round 2 (Stage under `makepad run`); each = rustc.
//! 1. libloading os/unix/mod.rs: `cfg_if!` in statement position. Its expansion is
//!    `#[cfg(..)] $crate::cfg_if! { @__temp_group .. }` statements: a macro statement behind
//!    attributes, cfg-stripped before it expands (rustc_expand config.rs).
//! 2. stage faces.rs:412: `left_rank.cmp(right_rank)` with left_rank: &{integer} (match
//!    ergonomics on a closure's tuple pattern): rustc's by-value probe step on `&{integer}`
//!    takes the integers' `Ord::cmp(&self)` with Self = {integer}, not `impl Ord for &A`.

#[macro_export]
macro_rules! cfg_if2 {
    (
        if #[cfg( $($i_meta:tt)+ )] { $( $i_tokens:tt )* }
        $(
            else if #[cfg( $($ei_meta:tt)+ )] { $( $ei_tokens:tt )* }
        )*
        $(
            else { $( $e_tokens:tt )* }
        )?
    ) => {
        $crate::cfg_if2! {
            @__items () ;
            (( $($i_meta)+ ) ( $( $i_tokens )* )),
            $(
                (( $($ei_meta)+ ) ( $( $ei_tokens )* )),
            )*
            $(
                (() ( $( $e_tokens )* )),
            )?
        }
    };

    // Internal and recursive macro to emit all the items
    //
    // Collects all the previous cfgs in a list at the beginning, so they can be
    // negated. After the semicolon are all the remaining items.
    (@__items ( $( ($($_:tt)*) , )* ) ; ) => {};
    (
        @__items ( $( ($($no:tt)+) , )* ) ;
        (( $( $($yes:tt)+ )? ) ( $( $tokens:tt )* )),
        $( $rest:tt , )*
    ) => {
        // Emit all items within one block, applying an appropriate #[cfg]. The
        // #[cfg] will require all `$yes` matchers specified and must also negate
        // all previous matchers.
        #[cfg(all(
            $( $($yes)+ , )?
            not(any( $( $($no)+ ),* ))
        ))]
        // Subtle: You might think we could put `$( $tokens )*` here. But if
        // that contains multiple items then the `#[cfg(all(..))]` above would
        // only apply to the first one. By wrapping `$( $tokens )*` in this
        // macro call, we temporarily group the items into a single thing (the
        // macro call) that will be included/excluded by the `#[cfg(all(..))]`
        // as appropriate. If the `#[cfg(all(..))]` succeeds, the macro call
        // will be included, and then evaluated, producing `$( $tokens )*`. See
        // also the "issue #90" test below.
        $crate::cfg_if2! { @__temp_group $( $tokens )* }

        // Recurse to emit all other items in `$rest`, and when we do so add all
        // our `$yes` matchers to the list of `$no` matchers as future emissions
        // will have to negate everything we just matched as well.
        $crate::cfg_if2! {
            @__items ( $( ($($no)+) , )* $( ($($yes)+) , )? ) ;
            $( $rest , )*
        }
    };

    // See the "Subtle" comment above.
    (@__temp_group $( $tokens:tt )* ) => {
        $( $tokens )*
    };
}


struct Lib(u32);
impl Lib {
    fn single(&self) -> Result<u32, ()> { Ok(self.0) }
    // the libloading shape: a generic fn, a fn-local extern crate, the macro as the body's tail
    fn get<T>(&self, _t: T) -> Result<u32, ()> {
        extern crate core;
        cfg_if2! {
            if #[cfg(any(target_os = "linux", target_os = "macos"))] {
                self.single()
            } else {
                self.this_fn_does_not_exist(|| Err(()))
            }
        }
    }
}

fn stmts() -> u32 {
    let mut n = 1;
    cfg_if2! {
        if #[cfg(target_os = "nonexistent")] {
            n += 100;
        } else if #[cfg(any(target_os = "linux", target_os = "macos"))] {
            let k = 10;
            n += k;
        } else {
            n += 1000;
        }
    }
    n
}

#[test]
fn cfg_if_statement_position() {
    assert_eq!(Lib(7).get(0u8), Ok(7));
    assert_eq!(stmts(), 11);
}

struct Choice { id: String }

fn ranked(available: bool) -> Vec<Choice> {
    let mut choices = Vec::new();
    choices.push((if available { 0 } else { 2 }, Choice { id: "b".to_string() }));
    choices.push((1, Choice { id: "a".to_string() }));
    choices.push((-1, Choice { id: "c".to_string() }));
    choices.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank.cmp(right_rank).then_with(|| left.id.cmp(&right.id))
    });
    choices.into_iter().map(|(_, c)| c).collect()
}

#[test]
fn int_var_cmp_through_refs() {
    let v: Vec<String> = ranked(false).into_iter().map(|c| c.id).collect();
    assert_eq!(v, ["c", "a", "b"]);
    let w: Vec<String> = ranked(true).into_iter().map(|c| c.id).collect();
    assert_eq!(w, ["c", "b", "a"]);
    let x = 5;
    let r = &x;
    assert_eq!(r.cmp(&&3), std::cmp::Ordering::Greater);
    assert_eq!(r.max(&7), &7);
}
