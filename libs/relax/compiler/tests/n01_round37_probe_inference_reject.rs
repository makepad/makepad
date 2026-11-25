// rejects: E0790
// (an associated function called on the trait without its impl type: rustc_trait_selection
// error_reporting/traits/ambiguity.rs maybe_report_ambiguity)
// rustc_hir_typeck/src/method/probe.rs:640-794,895-907: later autoderef
// inference must not constrain the original receiver of an inherent method.
use std::ops::Deref;
trait Member { fn view(&self) -> &u8; }
impl Member for u8 { fn view(&self) -> &u8 { self } }
struct Wrap<T>(T);
impl<T: Member> Deref for Wrap<T> { type Target = u8; fn deref(&self) -> &u8 { self.0.view() } }
impl<T> Wrap<T> { fn own(self) {} }
fn main() { let value = Wrap(Default::default()); value.own(); }
