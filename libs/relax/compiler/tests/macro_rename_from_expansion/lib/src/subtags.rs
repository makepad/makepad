impl_tinystr_subtag!(Subtag, subtags, subtag, subtags_subtag, 2);

pub mod script {
    impl_tinystr_subtag!(Script, subtags::script, script, subtags_script, 4);
    pub const COMMON_OR_LATIN: [Script; 2] = [script!("Zyyy"), script!("Latn")];
}
pub use script::{script, Script};

pub const UNKNOWN: Subtag = subtag!("und");
