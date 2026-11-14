// `extern { static __ImageBase }`, which a PE linker defines in every image (its DOS header),
// read and taken by address as winit's get_instance_handle takes its HINSTANCE. A test rustc
// builds is in the executable, whose base GetModuleHandleW(NULL) names; under Relax the program
// is in no image and reads the executable's (relax issue 187: not found, every winit app panicked
// at its first window). Other systems have no PE image base: nothing to read there.
#[cfg(windows)]
mod pe {
    #[repr(C)]
    pub struct ImageDosHeader {
        pub e_magic: u16,
    }

    extern "C" {
        pub static __ImageBase: ImageDosHeader;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> *mut u8;
    }
}

#[test]
fn the_image_base_is_the_executables() {
    #[cfg(windows)]
    unsafe {
        assert_eq!(pe::__ImageBase.e_magic, 0x5a4d, "the DOS header's MZ");
        let base = &pe::__ImageBase as *const pe::ImageDosHeader as *mut u8;
        assert_eq!(base, pe::GetModuleHandleW(std::ptr::null()), "the executable's image");
    }
}
