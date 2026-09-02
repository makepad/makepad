#[cfg(feature = "demo")]
mod demo;
#[cfg(feature = "native")]
mod native;

#[cfg(feature = "demo")]
pub use demo::*;
#[cfg(feature = "native")]
pub use native::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisioningProfile {
    Native,
    Demo,
}

pub const fn selected_profile(native: bool) -> ProvisioningProfile {
    if native {
        ProvisioningProfile::Native
    } else {
        ProvisioningProfile::Demo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioner_selection_is_explicit() {
        assert_eq!(selected_profile(true), ProvisioningProfile::Native);
        assert_eq!(selected_profile(false), ProvisioningProfile::Demo);
        assert_eq!(PROFILE, selected_profile(cfg!(feature = "native")));
    }
}
