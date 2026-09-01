use makepad_micro_serde::{DeBin, DeBinErr, SerBin};
use std::fmt;
use std::marker::PhantomData;

/// Persistent logical identity, serialized as a 128-bit `(actor, counter)` pair.
pub struct Id<K> {
    actor: u64,
    counter: u64,
    marker: PhantomData<fn() -> K>,
}

impl<K> Id<K> {
    pub const fn new(actor: u64, counter: u64) -> Self {
        Self {
            actor,
            counter,
            marker: PhantomData,
        }
    }

    pub const fn actor(self) -> u64 {
        self.actor
    }

    pub const fn counter(self) -> u64 {
        self.counter
    }

    pub const fn raw(self) -> (u64, u64) {
        (self.actor, self.counter)
    }
}

impl<K> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Id<K> {}

impl<K> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:016x}:{:016x})", self.actor, self.counter)
    }
}

impl<K> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.raw() == other.raw()
    }
}

impl<K> Eq for Id<K> {}

impl<K> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<K> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.raw().cmp(&other.raw())
    }
}

impl<K> std::hash::Hash for Id<K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.actor.hash(state);
        self.counter.hash(state);
    }
}

impl<K> SerBin for Id<K> {
    fn ser_bin(&self, output: &mut Vec<u8>) {
        self.actor.ser_bin(output);
        self.counter.ser_bin(output);
    }
}

impl<K> DeBin for Id<K> {
    fn de_bin(offset: &mut usize, input: &[u8]) -> Result<Self, DeBinErr> {
        Ok(Self::new(
            u64::de_bin(offset, input)?,
            u64::de_bin(offset, input)?,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdError {
    Exhausted,
}

/// Monotonic per-actor identity allocation. Actor creation is an application concern.
#[derive(Clone, Debug, Eq, PartialEq, SerBin, DeBin)]
pub struct IdGenerator {
    actor: u64,
    next_counter: u64,
}

impl IdGenerator {
    pub const fn new(actor: u64) -> Self {
        Self {
            actor,
            next_counter: 1,
        }
    }

    pub fn next<K>(&mut self) -> Result<Id<K>, IdError> {
        let counter = self.next_counter;
        self.next_counter = counter.checked_add(1).ok_or(IdError::Exhausted)?;
        Ok(Id::new(self.actor, counter))
    }

    pub const fn actor(&self) -> u64 {
        self.actor
    }
}

macro_rules! tags {
    ($(($tag:ident, $alias:ident)),+ $(,)?) => {
        $(
            #[derive(Debug)]
            pub enum $tag {}
            pub type $alias = Id<$tag>;
        )+
    };
}

tags!(
    (PartTag, PartId),
    (StaffTag, StaffId),
    (VoiceTag, VoiceId),
    (MeasureTag, MeasureId),
    (EventTag, EventId),
    (NoteTag, NoteId),
    (SpannerTag, SpannerId),
    (AnnotationTag, AnnotationId),
    (LayerTag, LayerId),
    (PartViewTag, PartViewId),
    (SourceRegionTag, SourceRegionId),
);
