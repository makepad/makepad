// time's fmt_component_match: nested expr forwarding, negative const turbofish,
// a qualified struct pattern inside a tuple variant, and extra arms as tt.
// rustc_parse parser/expr.rs: parse_dot_suffix_expr/parse_expr_path_start;
// parser/pat.rs: parse_pat_tuple_struct; rustc_expand mbe/transcribe.rs: transcribe_pnr.
macro_rules! try_likely_ok {
    ($e:expr) => {
        match $e {
            Ok(value) => value,
            Err(error) => {
                $crate::cold_path();
                return Err(error.into());
            }
        }
    };
}
fn cold_path() {}

mod error {
    pub struct ComponentRange;
    impl ComponentRange { pub fn conditional(_: &str) {} }
}
mod modifier { pub struct End { pub trailing_input: bool } }

struct Value(i32);
impl Value {
    const SUPPLIES_DATE: bool = true;
    fn calendar_year(&self, _: &mut ()) -> Self { Self(self.0) }
    fn narrow<const MIN: i32, const MAX: i32>(&self) -> Option<i32> {
        if self.0 >= MIN && self.0 <= MAX { Some(self.0) } else { None }
    }
}
struct Ranged<const MIN: i32, const MAX: i32>;
impl<const MIN: i32, const MAX: i32> Ranged<MIN, MAX> {
    unsafe fn new_unchecked(value: i32) -> Value { Value(value) }
}
fn fmt_year(output: &mut i32, year: i32) -> Result<i32, ()> {
    *output += year;
    Ok(year)
}

macro_rules! fmt_component_match {
    ($self:expr, $output:ident, $value:ident, $state:ident, $($extra:tt)*) => {
        match $self {
            Self::Year if V::SUPPLIES_DATE => {
                fmt_year(
                    $output,
                    try_likely_ok!(
                        $value.calendar_year($state).narrow::<-9_999, 9_999>()
                            .ok_or_else(|| error::ComponentRange::conditional("year"))
                    ).into(),
                ).map_err(Into::into)
            }
            Self::Century if V::SUPPLIES_DATE => {
                let year = $value.calendar_year($state);
                let century = unsafe { Ranged::<-9_999, 9_999>::new_unchecked(year.0 / 100) };
                fmt_year(
                    $output,
                    century.narrow::<-99, 99>()
                        .ok_or_else(|| error::ComponentRange::conditional("year"))?.into(),
                ).map_err(Into::into)
            }
            Self::End(modifier::End { trailing_input: _ }) => Ok(0),
            $($extra)*
        }
    };
}

enum Component { Year, Century, End(modifier::End), Literal(i32) }
impl Component {
    fn format(&self, output: &mut i32, value: &Value, state: &mut ()) -> Result<i32, ()> {
        type V = Value;
        fmt_component_match! { &self, output, value, state,
            Self::Literal(value) => Ok(*value),
            #[allow(unreachable_patterns)]
            Self::Year | Self::Century => Ok(-1),
        }
    }
}

#[test]
fn component_match_paths_and_nested_expansion() {
    let mut output = 0;
    let value = Value(2024);
    assert_eq!(Component::Year.format(&mut output, &value, &mut ()), Ok(2024));
    assert_eq!(Component::Century.format(&mut output, &value, &mut ()), Ok(20));
    assert_eq!(Component::Literal(5).format(&mut output, &value, &mut ()), Ok(5));
    let end = modifier::End { trailing_input: false };
    assert!(!end.trailing_input);
    assert_eq!(Component::End(end).format(&mut output, &value, &mut ()), Ok(0));
    assert_eq!(Component::Year.format(&mut output, &Value(10_000), &mut ()), Err(()));
}
