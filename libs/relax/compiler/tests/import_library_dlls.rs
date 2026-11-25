// `#[link(name = "mincore")]` names an import library of the Windows SDK whose imports live in 209
// DLLs, none of them mincore.dll: the linker reads mincore.lib to bind each import to its DLL, and
// Relax reads it the same way (os/implib.rs, os/proc.rs `find_imported`). Nothing else in the
// process loads these DLLs: the compression API is Cabinet.dll's, imported by ordinal;
// ConvertLengthToIpv4Mask is IPHLPAPI.DLL's and DnsNameCompare_W DNSAPI.dll's, imported by name.
#![cfg(windows)]

use core::ffi::c_void;

#[link(name = "mincore")]
extern "system" {
    fn CreateCompressor(algorithm: u32, routines: *const c_void, handle: *mut *mut c_void) -> i32;
    fn Compress(handle: *mut c_void, data: *const u8, size: usize, out: *mut u8, out_size: usize, written: *mut usize) -> i32;
    fn CloseCompressor(handle: *mut c_void) -> i32;
    fn CreateDecompressor(algorithm: u32, routines: *const c_void, handle: *mut *mut c_void) -> i32;
    fn Decompress(handle: *mut c_void, data: *const u8, size: usize, out: *mut u8, out_size: usize, written: *mut usize) -> i32;
    fn CloseDecompressor(handle: *mut c_void) -> i32;
    fn ConvertLengthToIpv4Mask(length: u32, mask: *mut u32) -> u32;
    fn DnsNameCompare_W(a: *const u16, b: *const u16) -> i32;
}

const XPRESS: u32 = 3;

#[test]
fn imports_by_ordinal_bind_to_their_dll() {
    let data = b"relax binds what the linker binds; ".repeat(40);
    let (mut packed, mut back) = (vec![0u8; data.len() + 64], vec![0u8; data.len()]);
    let (mut n, mut m) = (0, 0);
    unsafe {
        let mut c = std::ptr::null_mut();
        assert_ne!(CreateCompressor(XPRESS, std::ptr::null(), &mut c), 0, "CreateCompressor");
        assert_ne!(Compress(c, data.as_ptr(), data.len(), packed.as_mut_ptr(), packed.len(), &mut n), 0, "Compress");
        assert_ne!(CloseCompressor(c), 0, "CloseCompressor");
        let mut d = std::ptr::null_mut();
        assert_ne!(CreateDecompressor(XPRESS, std::ptr::null(), &mut d), 0, "CreateDecompressor");
        assert_ne!(Decompress(d, packed.as_ptr(), n, back.as_mut_ptr(), back.len(), &mut m), 0, "Decompress");
        assert_ne!(CloseDecompressor(d), 0, "CloseDecompressor");
    }
    println!("| {} bytes packed into {n}, unpacked into {m}", data.len());
    assert!(n < data.len() / 4, "a repetitive text packs small: {n} bytes");
    assert_eq!(&back[..m], &data[..], "the round trip gives the text back");
}

#[test]
fn imports_by_name_bind_to_their_dll() {
    let mut mask = 0u32;
    let status = unsafe { ConvertLengthToIpv4Mask(20, &mut mask) };
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (a, b, c) = (wide("Relax.Example"), wide("relax.example"), wide("relay.example"));
    let (same, other) = unsafe { (DnsNameCompare_W(a.as_ptr(), b.as_ptr()), DnsNameCompare_W(a.as_ptr(), c.as_ptr())) };
    println!("| mask status {status} bytes {:?}, names equal {same} {other}", mask.to_ne_bytes());
    assert_eq!((status, mask.to_ne_bytes()), (0, [255, 255, 240, 0]), "a 20-bit mask, in network order");
    assert!(same != 0 && other == 0, "DNS names compare without case: {same} {other}");
}
