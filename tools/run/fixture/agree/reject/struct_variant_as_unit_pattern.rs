// rejects: E0533
// rejects: expected unit struct, unit variant or constant, found struct variant
// rejects-at: 15:9 E0533
// A struct variant named by a bare path pattern, as if it were a unit variant: rustc refuses the
// pattern (E0533); it must not lower as a match on the variant alone.
pub enum Role {
    Host,
    Viewer { sharer: u32 },
}

pub fn label(role: &Role) -> &'static str {
    match role {
        Role::Host => "host",
        // the struct variant needs `Role::Viewer { .. }`
        Role::Viewer => "viewer",
    }
}
