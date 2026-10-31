//! Constructors and constructor sets of pattern analysis (rustc_pattern_analysis constructor.rs):
//! what a pattern's head is (a variant, a struct, an integer range, a slice length, ..), the
//! constructors a type has, and splitting a column's constructors into the ones present and the
//! ones missing.

use std::cmp::{max, min, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Presence {
    Unseen,
    Seen,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RangeEnd {
    Included,
    Excluded,
}

/// An integer that may be infinite; finite values are stored biased so that the order of the
/// encoding is the order of the (signed or unsigned) values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaybeInfiniteInt {
    NegInfinity,
    Finite(u128),
    PosInfinity,
}

impl MaybeInfiniteInt {
    pub fn new_finite_uint(bits: u128) -> Self {
        MaybeInfiniteInt::Finite(bits)
    }
    /// `bits`: the value's two's complement bits of `size` bits
    pub fn new_finite_int(bits: u128, size: u32) -> Self {
        let bias = 1u128 << (size - 1);
        MaybeInfiniteInt::Finite(bits ^ bias)
    }
    pub fn as_finite_uint(self) -> Option<u128> {
        match self {
            MaybeInfiniteInt::Finite(bits) => Some(bits),
            _ => None,
        }
    }
    pub fn as_finite_int(self, size: u32) -> Option<u128> {
        match self {
            MaybeInfiniteInt::Finite(bits) => Some(bits ^ (1u128 << (size - 1))),
            _ => None,
        }
    }
    pub fn minus_one(self) -> Option<Self> {
        match self {
            MaybeInfiniteInt::Finite(n) => n.checked_sub(1).map(MaybeInfiniteInt::Finite),
            x => Some(x),
        }
    }
    pub fn plus_one(self) -> Option<Self> {
        match self {
            MaybeInfiniteInt::Finite(n) => match n.checked_add(1) {
                Some(m) => Some(MaybeInfiniteInt::Finite(m)),
                None => Some(MaybeInfiniteInt::PosInfinity),
            },
            x => Some(x),
        }
    }
}

/// A half-open range `lo..hi` of integers (chars are integers here too).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IntRange {
    pub lo: MaybeInfiniteInt,
    pub hi: MaybeInfiniteInt,
}

impl IntRange {
    pub fn is_singleton(&self) -> bool {
        self.lo.plus_one() == Some(self.hi)
    }
    pub fn from_singleton(x: MaybeInfiniteInt) -> IntRange {
        IntRange { lo: x, hi: x.plus_one().unwrap() }
    }
    /// None: an empty range (`5..5`, `5..=4`: an error rustc reports before the analysis)
    pub fn from_range(lo: MaybeInfiniteInt, mut hi: MaybeInfiniteInt, end: RangeEnd) -> Option<IntRange> {
        if end == RangeEnd::Included {
            hi = hi.plus_one()?;
        }
        if lo >= hi {
            return None;
        }
        Some(IntRange { lo, hi })
    }
    pub fn is_subrange(&self, other: &Self) -> bool {
        other.lo <= self.lo && self.hi <= other.hi
    }
    fn intersection(&self, other: &Self) -> Option<Self> {
        if self.lo < other.hi && other.lo < self.hi {
            Some(IntRange { lo: max(self.lo, other.lo), hi: min(self.hi, other.hi) })
        } else {
            None
        }
    }

    /// Partitions `self` into the ranges no column range straddles, each marked seen (covered
    /// by some column range) or unseen.
    fn split(&self, column_ranges: impl Iterator<Item = IntRange>) -> Vec<(Presence, IntRange)> {
        let mut boundaries: Vec<(MaybeInfiniteInt, isize)> = column_ranges
            .filter_map(|r| self.intersection(&r))
            .flat_map(|r| [(r.lo, 1), (r.hi, -1)])
            .collect();
        boundaries.sort_unstable();
        let mut out = Vec::new();
        let mut paren_counter = 0isize;
        let mut prev_bdy = self.lo;
        for (bdy, delta) in boundaries.into_iter().chain(std::iter::once((self.hi, 0))) {
            if prev_bdy != bdy {
                let presence = if paren_counter > 0 { Presence::Seen } else { Presence::Unseen };
                out.push((presence, IntRange { lo: prev_bdy, hi: bdy }));
            }
            prev_bdy = bdy;
            paren_counter += delta;
        }
        out
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SliceKind {
    /// `[a, b, c]`
    FixedLen(usize),
    /// `[a, .., b]`: (prefix, suffix)
    VarLen(usize, usize),
}

impl SliceKind {
    pub fn arity(self) -> usize {
        match self {
            SliceKind::FixedLen(length) => length,
            SliceKind::VarLen(prefix, suffix) => prefix + suffix,
        }
    }
    fn covers_length(self, other_len: usize) -> bool {
        match self {
            SliceKind::FixedLen(len) => len == other_len,
            SliceKind::VarLen(prefix, suffix) => prefix + suffix <= other_len,
        }
    }
}

/// A slice or array pattern's shape.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    /// Some: the array's length
    pub array_len: Option<usize>,
    pub kind: SliceKind,
}

impl Slice {
    /// None: a pattern longer than its array (an error typeck reports)
    pub fn new(array_len: Option<usize>, kind: SliceKind) -> Option<Self> {
        let kind = match (array_len, kind) {
            (Some(len), SliceKind::VarLen(prefix, suffix)) if prefix + suffix == len => SliceKind::FixedLen(len),
            (Some(len), SliceKind::VarLen(prefix, suffix)) if prefix + suffix > len => return None,
            _ => kind,
        };
        Some(Slice { array_len, kind })
    }
    pub fn arity(self) -> usize {
        self.kind.arity()
    }
    fn is_covered_by(self, other: Self) -> bool {
        other.kind.covers_length(self.arity())
    }

    /// The lengths a column of slice patterns must be split into: the fixed lengths below the
    /// longest variable-length pattern, then that variable length (constructor.rs Slice::split).
    fn split(self, column_slices: impl Iterator<Item = Slice>) -> Vec<(Presence, Slice)> {
        let smaller_lengths;
        let arity = self.arity();
        let mut max_slice = self.kind;
        let mut min_var_len = usize::MAX;
        let mut seen_fixed_lens: Vec<usize> = Vec::new();
        match &mut max_slice {
            SliceKind::VarLen(max_prefix_len, max_suffix_len) => {
                let mut fixed_len_upper_bound = 1;
                for slice in column_slices {
                    match slice.kind {
                        SliceKind::FixedLen(len) => {
                            fixed_len_upper_bound = max(fixed_len_upper_bound, len + 1);
                            seen_fixed_lens.push(len);
                        }
                        SliceKind::VarLen(prefix, suffix) => {
                            *max_prefix_len = max(*max_prefix_len, prefix);
                            *max_suffix_len = max(*max_suffix_len, suffix);
                            min_var_len = min(min_var_len, prefix + suffix);
                        }
                    }
                }
                if let Some(delta) = fixed_len_upper_bound.checked_sub(*max_prefix_len + *max_suffix_len) {
                    *max_prefix_len += delta
                }
                if let Some(len) = self.array_len {
                    if max_slice.arity() >= len {
                        max_slice = SliceKind::FixedLen(len);
                    }
                }
                smaller_lengths = match self.array_len {
                    Some(_) => 0..0,
                    None => self.arity()..max_slice.arity(),
                };
            }
            SliceKind::FixedLen(_) => {
                for slice in column_slices {
                    match slice.kind {
                        SliceKind::FixedLen(len) => {
                            if len == arity {
                                seen_fixed_lens.push(len);
                            }
                        }
                        SliceKind::VarLen(prefix, suffix) => {
                            min_var_len = min(min_var_len, prefix + suffix);
                        }
                    }
                }
                smaller_lengths = 0..0;
            }
        };
        smaller_lengths.map(SliceKind::FixedLen).chain(std::iter::once(max_slice)).map(|kind| {
            let arity = kind.arity();
            let seen = if min_var_len <= arity || seen_fixed_lens.contains(&arity) { Presence::Seen } else { Presence::Unseen };
            (seen, Slice::new(self.array_len, kind).expect("split slice fits its array"))
        }).collect()
    }
}

/// The head of a pattern, or of a value's shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Ctor {
    /// a struct, tuple, or a union's single field set
    Struct,
    /// an enum's variant (by index)
    Variant(u32),
    /// `&` / `&mut`
    Ref,
    Slice(Slice),
    /// a union pattern (one or more fields)
    UnionField,
    Bool(bool),
    /// integers and chars
    IntRange(IntRange),
    /// f32 / f64 values and ranges, compared as f64 (every f32 is exact in it)
    FloatRange(f64, f64, RangeEnd),
    /// a string literal (the `str` under the pattern's `&`)
    Str(String),
    /// a constant the analysis cannot look into (matches nothing else, nothing covers it)
    Opaque(u32),
    /// an or-pattern; its fields are the alternatives
    Or,
    Wildcard,
    /// a value of an empty type (witnesses only)
    Never,
    /// what `#[non_exhaustive]` and unlistable types leave out (witnesses only)
    NonExhaustive,
    /// hidden variants (witnesses only)
    Hidden,
    /// the constructors missing from a column, taken as one
    Missing,
    /// a private field of an uninhabited type: skipped
    PrivateUninhabited,
}

impl Ctor {
    pub fn is_non_exhaustive(&self) -> bool {
        matches!(self, Ctor::NonExhaustive)
    }
    fn as_variant(&self) -> Option<u32> {
        match self {
            Ctor::Variant(i) => Some(*i),
            _ => None,
        }
    }
    fn as_bool(&self) -> Option<bool> {
        match self {
            Ctor::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_int_range(&self) -> Option<&IntRange> {
        match self {
            Ctor::IntRange(r) => Some(r),
            _ => None,
        }
    }
    fn as_slice(&self) -> Option<Slice> {
        match self {
            Ctor::Slice(s) => Some(*s),
            _ => None,
        }
    }

    /// Whether every value this constructor matches is matched by `other` (constructors of
    /// the same type). None: incompatible constructors (a type error the analysis was given).
    pub fn is_covered_by(&self, other: &Self) -> Option<bool> {
        use Ctor::*;
        Some(match (self, other) {
            (Wildcard, _) => return None,
            (_, Wildcard) => true,
            (PrivateUninhabited, _) => true,
            (Missing | NonExhaustive | Hidden, _) => false,
            (Struct, Struct) => true,
            (Ref, Ref) => true,
            (UnionField, UnionField) => true,
            (Variant(a), Variant(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (IntRange(a), IntRange(b)) => a.is_subrange(b),
            (FloatRange(sf, st, se), FloatRange(of, ot, oe)) => {
                sf >= of && match st.partial_cmp(ot) {
                    Some(Ordering::Less) => true,
                    Some(Ordering::Equal) => oe == se,
                    _ => false,
                }
            }
            (Str(a), Str(b)) => a == b,
            (Slice(a), Slice(b)) => a.is_covered_by(*b),
            (Opaque(a), Opaque(b)) => a == b,
            (Opaque(..), _) | (_, Opaque(..)) => false,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantVisibility {
    Visible,
    /// `#[doc(hidden)]` foreign or unstable: never named in witnesses
    Hidden,
    /// uninhabited
    Empty,
}

/// The constructors of a type.
#[derive(Debug)]
pub enum ConstructorSet {
    /// a struct or tuple; `empty`: uninhabited
    Struct { empty: bool },
    Variants { variants: Vec<VariantVisibility>, non_exhaustive: bool },
    Ref,
    Union,
    Bool,
    /// integers (chars: two ranges around the surrogates)
    Integers { range_1: IntRange, range_2: Option<IntRange> },
    Slice { array_len: Option<usize>, subtype_is_empty: bool },
    /// floats, str, pointers, generics: only a wildcard covers them
    Unlistable,
    /// `!`, empty enums
    NoConstructors,
}

pub struct SplitConstructorSet {
    pub present: Vec<Ctor>,
    pub missing: Vec<Ctor>,
    pub missing_empty: Vec<Ctor>,
}

impl ConstructorSet {
    /// Splits the type's constructors by the column's: the ones present (refined so that no
    /// column constructor partially covers one), the ones missing, the missing empty ones.
    pub fn split<'a>(&self, ctors: impl Iterator<Item = &'a Ctor>) -> SplitConstructorSet {
        let mut present: Vec<Ctor> = Vec::new();
        let mut missing_empty = Vec::new();
        let mut missing = Vec::new();
        let mut seen = Vec::new();
        for ctor in ctors.cloned() {
            match ctor {
                Ctor::Opaque(..) => present.push(ctor),
                Ctor::Wildcard => {}
                _ => seen.push(ctor),
            }
        }
        match self {
            ConstructorSet::Struct { empty } => {
                if !seen.is_empty() {
                    present.push(Ctor::Struct);
                } else if *empty {
                    missing_empty.push(Ctor::Struct);
                } else {
                    missing.push(Ctor::Struct);
                }
            }
            ConstructorSet::Ref => {
                if !seen.is_empty() { present.push(Ctor::Ref) } else { missing.push(Ctor::Ref) }
            }
            ConstructorSet::Union => {
                if !seen.is_empty() { present.push(Ctor::UnionField) } else { missing.push(Ctor::UnionField) }
            }
            ConstructorSet::Variants { variants, non_exhaustive } => {
                let mut seen_set = vec![false; variants.len()];
                for idx in seen.iter().filter_map(|c| c.as_variant()) {
                    if let Some(s) = seen_set.get_mut(idx as usize) { *s = true; }
                }
                let mut skipped_a_hidden_variant = false;
                for (idx, visibility) in variants.iter().enumerate() {
                    let ctor = Ctor::Variant(idx as u32);
                    if seen_set[idx] {
                        present.push(ctor);
                    } else {
                        match visibility {
                            VariantVisibility::Visible => missing.push(ctor),
                            VariantVisibility::Hidden => skipped_a_hidden_variant = true,
                            VariantVisibility::Empty => missing_empty.push(ctor),
                        }
                    }
                }
                if skipped_a_hidden_variant {
                    missing.push(Ctor::Hidden);
                }
                if *non_exhaustive {
                    missing.push(Ctor::NonExhaustive);
                }
            }
            ConstructorSet::Bool => {
                let (mut seen_false, mut seen_true) = (false, false);
                for b in seen.iter().filter_map(|c| c.as_bool()) {
                    if b { seen_true = true } else { seen_false = true }
                }
                if seen_true { present.push(Ctor::Bool(true)) } else { missing.push(Ctor::Bool(true)) }
                if seen_false { present.push(Ctor::Bool(false)) } else { missing.push(Ctor::Bool(false)) }
            }
            ConstructorSet::Integers { range_1, range_2 } => {
                let seen_ranges: Vec<IntRange> = seen.iter().filter_map(|c| c.as_int_range()).copied().collect();
                for (seen, r) in range_1.split(seen_ranges.iter().copied()) {
                    match seen {
                        Presence::Unseen => missing.push(Ctor::IntRange(r)),
                        Presence::Seen => present.push(Ctor::IntRange(r)),
                    }
                }
                if let Some(range_2) = range_2 {
                    for (seen, r) in range_2.split(seen_ranges.iter().copied()) {
                        match seen {
                            Presence::Unseen => missing.push(Ctor::IntRange(r)),
                            Presence::Seen => present.push(Ctor::IntRange(r)),
                        }
                    }
                }
            }
            ConstructorSet::Slice { array_len, subtype_is_empty } => {
                let seen_slices = seen.iter().filter_map(|c| c.as_slice());
                let base_slice = Slice::new(*array_len, SliceKind::VarLen(0, 0)).expect("empty slice pattern fits");
                for (seen, s) in base_slice.split(seen_slices) {
                    let ctor = Ctor::Slice(s);
                    match seen {
                        Presence::Seen => present.push(ctor),
                        Presence::Unseen => {
                            if *subtype_is_empty && s.arity() != 0 { missing_empty.push(ctor) } else { missing.push(ctor) }
                        }
                    }
                }
            }
            ConstructorSet::Unlistable => {
                present.extend(seen);
                missing.push(Ctor::NonExhaustive);
            }
            ConstructorSet::NoConstructors => {
                missing_empty.push(Ctor::Never);
            }
        }
        SplitConstructorSet { present, missing, missing_empty }
    }

    /// Every constructor is empty (the type is uninhabited).
    pub fn all_empty(&self) -> bool {
        match self {
            ConstructorSet::Bool | ConstructorSet::Integers { .. } | ConstructorSet::Ref | ConstructorSet::Union | ConstructorSet::Unlistable => false,
            ConstructorSet::NoConstructors => true,
            ConstructorSet::Struct { empty } => *empty,
            ConstructorSet::Variants { variants, non_exhaustive } => {
                !*non_exhaustive && variants.iter().all(|v| matches!(v, VariantVisibility::Empty))
            }
            ConstructorSet::Slice { array_len, subtype_is_empty } => *subtype_is_empty && matches!(array_len, Some(1..)),
        }
    }
}
