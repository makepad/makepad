//! Protocol schema types; the tables themselves are generated into `proto.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgType {
    Int,
    Uint,
    Fixed,
    Str,
    StrOpt,
    Array,
    Fd,
    /// Existing object of this interface ("" = any).
    Object(&'static str),
    ObjectOpt(&'static str),
    /// New object of this interface, id chosen by the sender.
    NewId(&'static str),
    /// `string interface, uint version, new_id` (wl_registry.bind).
    NewIdDyn,
}
pub struct Msg {
    pub name: &'static str,
    pub since: u32,
    pub destructor: bool,
    pub args: &'static [ArgType],
}
pub struct Interface {
    pub name: &'static str,
    pub version: u32,
    pub requests: &'static [Msg],
    pub events: &'static [Msg],
}
impl core::fmt::Debug for Interface {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result { write!(f, "{}@v{}", self.name, self.version) }
}
/// Looks an interface up by its wire name.
pub fn find(name: &str) -> Option<&'static Interface> {
    crate::proto::ALL.iter().copied().find(|i| i.name == name)
}
