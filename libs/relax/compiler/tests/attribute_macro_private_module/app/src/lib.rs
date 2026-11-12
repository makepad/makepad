use apm_dep::apm_dep;

#[apm_dep::generate]
pub struct Original;

#[test]
fn macro_import_does_not_import_a_private_module_or_hide_the_crate() {
    let _: Generated = Generated;
    assert_eq!(apm_dep!(), 17);
}

mod public_import {
    use apm_dep::public_module as attrs;
    #[attrs::generate]
    pub struct Original;

    #[test]
    fn a_public_module_alias_still_resolves_attributes() {
        let _: Generated = Generated;
    }
}
