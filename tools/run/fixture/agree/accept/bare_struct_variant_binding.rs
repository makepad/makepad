enum Role {
    Host,
    Viewer { sharer: u32 },
}

#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_imports)]
pub fn bare_viewer_is_irrefutable_and_readable() {
    use Role::Viewer;
    let is_host = match Role::Host {
        Viewer => matches!(Viewer, Role::Host),
    };
    assert!(is_host);
    let Viewer = Role::Host;
    assert!(matches!(Viewer, Role::Host));
}

#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_imports)]
pub fn bare_viewer_on_ref_keeps_reference_type() {
    use Role::Viewer;
    let r: &Role = &Role::Host;
    match r {
        Viewer => {
            let bound: &Role = Viewer;
            assert!(matches!(bound, Role::Host));
        }
    }
}

#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_imports)]
pub fn mut_ref_and_at_viewer_bind() {
    {
        use Role::Viewer;
        match Role::Host {
            mut Viewer => {
                assert!(matches!(Viewer, Role::Host));
                Viewer = Role::Viewer { sharer: 9 };
                assert!(matches!(Viewer, Role::Viewer { sharer } if sharer == 9));
            }
        }
        match Role::Host {
            ref Viewer => {
                let bound: &Role = Viewer;
                assert!(matches!(bound, Role::Host));
            }
        }
    }
    {
        use Role::*;
        match Host {
            Viewer @ _ => assert!(matches!(Viewer, Role::Host)),
        }
    }
}

struct Record {
    x: u32,
}

#[cfg_attr(test, test)]
#[allow(non_snake_case)]
pub fn bare_braced_struct_binds() {
    let record = Record { x: 3 };
    let x = match record {
        Record => Record.x,
    };
    assert_eq!(x, 3);
    match (Record { x }) {
        mut Record => {
            Record.x += 1;
            assert_eq!(Record.x, 4);
        }
    }
}

mod a {
    pub enum A { Same { x: u32 }, Other }
}
mod b {
    pub enum B { Same { y: u32 } }
}

#[cfg_attr(test, test)]
#[allow(non_snake_case, unused_imports)]
pub fn ambiguous_glob_struct_variants_still_bind() {
    use a::A::*;
    use b::B::*;
    let is_other = match a::A::Other {
        Same => matches!(Same, a::A::Other),
    };
    assert!(is_other);
    let a = a::A::Same { x: 3 };
    assert!(matches!(a, a::A::Same { x } if x == 3));
    let b::B::Same { y } = b::B::Same { y: 4 };
    assert_eq!(y, 4);
}

#[cfg_attr(test, test)]
pub fn unit_variant_and_ref_const_still_match() {
    use Role::Host;
    let tag = match &Role::Host {
        Host => 1,
        _ => 2,
    };
    assert_eq!(tag, 1);
    const REF_UNIT: &() = &();
    let unit = match &() { REF_UNIT => 1 };
    assert_eq!(unit, 1);
}
