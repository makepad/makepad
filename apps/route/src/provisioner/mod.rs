pub mod demo;
#[cfg(not(feature = "demo"))]
mod native;

#[cfg(feature = "demo")]
pub use demo::*;
#[cfg(not(feature = "demo"))]
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
        // The native data plane serves every profile but the hosted demo.
        assert_eq!(PROFILE, selected_profile(!cfg!(feature = "demo")));
    }
}
