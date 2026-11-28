enum Role {
    Host,
    Viewer { sharer: u32 },
    EmptyStruct {},
}

fn sharer(role: &Role) -> Option<u32> {
    match role {
        Role::Viewer { sharer } => Some(*sharer),
        _ => None,
    }
}

// Keep the matches visible to package check as well as both test harnesses.
#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_variables, unused_imports, unreachable_patterns)]
pub fn bare_single_import_binds_host() {
    use Role::Viewer;
    let classify = |role: Role| match role {
        Viewer => 1,
        _ => 2,
    };
    assert_eq!(classify(Role::Host), 1);
    let viewer = Role::Viewer { sharer: 7 };
    assert_eq!(sharer(&viewer), Some(7));
    assert_eq!(classify(viewer), 1);
}

#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_variables, unused_imports, unreachable_patterns)]
pub fn bare_glob_import_binds_host() {
    use Role::*;
    let got = match Role::Host {
        Viewer => 1,
        _ => 2,
    };
    assert_eq!(got, 1);
}

#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_variables, unused_imports, unreachable_patterns)]
pub fn bare_empty_struct_variant_binds_host() {
    use Role::EmptyStruct;
    assert_eq!(sharer(&Role::EmptyStruct {}), None);
    let got = match Role::Host {
        EmptyStruct => 1,
        _ => 2,
    };
    assert_eq!(got, 1);
}
