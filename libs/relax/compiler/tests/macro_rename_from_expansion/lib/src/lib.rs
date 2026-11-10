// icu_locale_core's layout: `extensions` is declared before the module whose expansion defines
// and renames the macros its files call.
#[macro_use]
mod helpers;

pub mod extensions;
#[macro_use]
pub mod subtags;
pub mod preferences;

pub fn sum() -> usize {
    extensions::private::X.0.len()
        + extensions::unicode::TRUE_VALUE.0.len()
        + extensions::unicode::FALSE_VALUE.0.len()
        + extensions::transform::TRUE_TVALUE.0.len()
        + extensions::transform::LATIN.0.len()
        + extensions::transform::latin().0.len()
        + preferences::CALENDAR[1].0.len()
        + subtags::UNKNOWN.0.len()
        + subtags::script::COMMON_OR_LATIN[0].0.len()
}
