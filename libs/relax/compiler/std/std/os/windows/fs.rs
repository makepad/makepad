//! std::os::windows::fs: Windows extension traits for std::fs and symlink creation (real std's
//! library/std/src/os/windows/fs.rs).

use crate::fs::{self, Metadata, OpenOptions, Permissions};
use crate::io;
use crate::path::Path;
use crate::time::SystemTime;

pub trait FileExt {
    /// Reads at `offset` (moves the file cursor, unlike unix's read_at).
    fn seek_read(&self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
    /// Writes at `offset` (moves the file cursor, unlike unix's write_at).
    fn seek_write(&self, buf: &[u8], offset: u64) -> io::Result<usize>;
}

impl FileExt for fs::File {
    fn seek_read(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.read_at(buf, offset)
    }
    fn seek_write(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.write_at(buf, offset)
    }
}

pub trait OpenOptionsExt {
    fn access_mode(&mut self, access: u32) -> &mut Self;
    fn share_mode(&mut self, val: u32) -> &mut Self;
    fn custom_flags(&mut self, flags: u32) -> &mut Self;
    fn attributes(&mut self, val: u32) -> &mut Self;
    fn security_qos_flags(&mut self, flags: u32) -> &mut Self;
}

impl OpenOptionsExt for OpenOptions {
    fn access_mode(&mut self, access: u32) -> &mut OpenOptions {
        self.set_access_mode(access);
        self
    }
    fn share_mode(&mut self, share: u32) -> &mut OpenOptions {
        self.set_share_mode(share);
        self
    }
    fn custom_flags(&mut self, flags: u32) -> &mut OpenOptions {
        self.set_custom_flags(flags);
        self
    }
    fn attributes(&mut self, attributes: u32) -> &mut OpenOptions {
        self.set_attributes(attributes);
        self
    }
    fn security_qos_flags(&mut self, flags: u32) -> &mut OpenOptions {
        self.set_security_qos_flags(flags);
        self
    }
}

pub trait PermissionsExt {
    fn file_attributes(&self) -> u32;
    fn set_file_attributes(&mut self, mask: u32);
    fn from_file_attributes(mask: u32) -> Self;
}

impl PermissionsExt for Permissions {
    fn file_attributes(&self) -> u32 {
        Permissions::file_attributes(self)
    }
    fn set_file_attributes(&mut self, mask: u32) {
        Permissions::set_file_attributes(self, mask)
    }
    fn from_file_attributes(mask: u32) -> Permissions {
        Permissions::from_file_attributes(mask)
    }
}

pub trait MetadataExt {
    fn file_attributes(&self) -> u32;
    /// 100 ns intervals since 1601-01-01 (a FILETIME).
    fn creation_time(&self) -> u64;
    fn last_access_time(&self) -> u64;
    fn last_write_time(&self) -> u64;
    fn file_size(&self) -> u64;
    /// None when the metadata came from a directory listing (as real std).
    fn volume_serial_number(&self) -> Option<u32>;
    fn number_of_links(&self) -> Option<u32>;
    fn file_index(&self) -> Option<u64>;
    fn change_time(&self) -> Option<u64>;
}

impl MetadataExt for Metadata {
    fn file_attributes(&self) -> u32 {
        self.attrs()
    }
    fn creation_time(&self) -> u64 {
        self.created_u64()
    }
    fn last_access_time(&self) -> u64 {
        self.accessed_u64()
    }
    fn last_write_time(&self) -> u64 {
        self.modified_u64()
    }
    fn file_size(&self) -> u64 {
        self.len()
    }
    fn volume_serial_number(&self) -> Option<u32> {
        Metadata::volume_serial_number(self)
    }
    fn number_of_links(&self) -> Option<u32> {
        Metadata::number_of_links(self)
    }
    fn file_index(&self) -> Option<u64> {
        Metadata::file_index(self)
    }
    fn change_time(&self) -> Option<u64> {
        self.changed_u64()
    }
}

pub trait FileTypeExt {
    fn is_symlink_dir(&self) -> bool;
    fn is_symlink_file(&self) -> bool;
}

impl FileTypeExt for fs::FileType {
    fn is_symlink_dir(&self) -> bool {
        fs::FileType::is_symlink_dir(self)
    }
    fn is_symlink_file(&self) -> bool {
        fs::FileType::is_symlink_file(self)
    }
}

pub trait FileTimesExt {
    fn set_created(self, t: SystemTime) -> Self;
}

impl FileTimesExt for fs::FileTimes {
    fn set_created(self, t: SystemTime) -> fs::FileTimes {
        self.with_created(t)
    }
}

/// A symlink to a file (CreateSymbolicLinkW; unprivileged creation in developer mode).
pub fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    fs::symlink_inner(original.as_ref(), link.as_ref(), false)
}

/// A symlink to a directory (SYMBOLIC_LINK_FLAG_DIRECTORY).
pub fn symlink_dir<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    fs::symlink_inner(original.as_ref(), link.as_ref(), true)
}
