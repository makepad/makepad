//! Handles and keys: what is animated (targets and properties), how things
//! are named (tags) and how built animations are referred to (tween handles).

use crate::splitmix64;

/// One animated object: GSAP's "target". The host decides what the number
/// means (a widget, a list item, a scene node); the engine only keys values by it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId(pub u32);

/// One animated property of a target: the key of a GSAP vars entry such as
/// `x` or `backgroundColor`. Hosts usually build it from an interned id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PropKey(pub u64);

impl PropKey {
    /// A key for a nested property path such as `draw_bg.color`.
    ///
    /// A one-part path is that part itself (`path(&[a]) == PropKey(a)`); longer
    /// paths fold every part through [`splitmix64`] starting from 0, so the
    /// order of the parts matters. Usable in `const` items.
    pub const fn path(parts: &[u64]) -> PropKey {
        if parts.len() == 1 {
            return PropKey(parts[0]);
        }
        let mut h = 0u64;
        let mut i = 0;
        while i < parts.len() {
            h = splitmix64(h ^ parts[i]);
            i += 1;
        }
        PropKey(h)
    }
}

/// A name: a timeline label, the identity of a callback, or a GSAP `id`
/// (`gsap.getById`). [`Tag::NONE`] means "unnamed".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tag(pub u64);

impl Tag {
    /// No tag.
    pub const NONE: Tag = Tag(0);
}

/// One value cell of the engine's value store: a (target, property) pair.
/// Tweens write it every frame; the host reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SlotId(pub u32);

/// A handle to a built animation: a tween, timeline, stagger group, delayed
/// call or pause (GSAP's `Animation` object).
///
/// A handle is generational: once its animation is freed every control call
/// made with it is a no-op and every getter answers the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TweenId {
    pub(crate) ix: u32,
    pub(crate) gen: u32,
}

impl Default for TweenId {
    /// [`TweenId::NONE`] (for `#[rust]` handle fields).
    fn default() -> Self {
        TweenId::NONE
    }
}

impl TweenId {
    /// The handle that refers to nothing.
    pub const NONE: TweenId = TweenId {
        ix: u32::MAX,
        gen: 0,
    };

    /// Whether this is [`TweenId::NONE`].
    #[inline]
    pub fn is_none(self) -> bool {
        self.ix == u32::MAX
    }

    /// The handle as one number (`generation << 32 | index`), for hosts that
    /// keep ids as plain data (a widget model, a script value).
    #[inline]
    pub const fn to_bits(self) -> u64 {
        ((self.gen as u64) << 32) | self.ix as u64
    }

    /// The handle [`TweenId::to_bits`] made. Any other number is a handle
    /// that refers to nothing (every call with it is a no-op).
    #[inline]
    pub const fn from_bits(b: u64) -> TweenId {
        TweenId {
            ix: b as u32,
            gen: (b >> 32) as u32,
        }
    }
}

/// A handle to a motion path stored in an engine
/// ([`crate::TweenEngine::add_path`]). Generational like [`TweenId`]: once
/// the path is freed the handle is stale and building with it adds nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PathId {
    pub(crate) ix: u32,
    pub(crate) gen: u32,
}

impl Default for PathId {
    /// [`PathId::NONE`].
    fn default() -> Self {
        PathId::NONE
    }
}

impl PathId {
    /// The handle that refers to no path.
    pub const NONE: PathId = PathId {
        ix: u32::MAX,
        gen: 0,
    };

    /// Whether this is [`PathId::NONE`].
    #[inline]
    pub fn is_none(self) -> bool {
        self.ix == u32::MAX
    }
}
