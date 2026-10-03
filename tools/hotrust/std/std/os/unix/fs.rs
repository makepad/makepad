//! unix fs extension traits and symlink().

use crate::fs::{self, DirBuilder, DirEntry, FileType, Metadata, OpenOptions, Permissions};
use crate::io;
use crate::path::Path;

pub trait PermissionsExt {
    fn mode(&self) -> u32;
    fn set_mode(&mut self, mode: u32);
    fn from_mode(mode: u32) -> Self;
}

impl PermissionsExt for Permissions {
    fn mode(&self) -> u32 {
        self.mode_bits()
    }
    fn set_mode(&mut self, mode: u32) {
        self.set_mode_bits(mode)
    }
    fn from_mode(mode: u32) -> Permissions {
        Permissions::from_mode_bits(mode)
    }
}

pub trait OpenOptionsExt {
    fn mode(&mut self, mode: u32) -> &mut Self;
    fn custom_flags(&mut self, flags: i32) -> &mut Self;
}

impl OpenOptionsExt for OpenOptions {
    fn mode(&mut self, mode: u32) -> &mut OpenOptions {
        self.set_mode(mode);
        self
    }
    fn custom_flags(&mut self, flags: i32) -> &mut OpenOptions {
        self.set_custom_flags(flags);
        self
    }
}

pub trait DirBuilderExt {
    fn mode(&mut self, mode: u32) -> &mut Self;
}

impl DirBuilderExt for DirBuilder {
    fn mode(&mut self, mode: u32) -> &mut DirBuilder {
        self.set_mode(mode);
        self
    }
}

pub trait MetadataExt {
    fn dev(&self) -> u64;
    fn ino(&self) -> u64;
    fn mode(&self) -> u32;
    fn nlink(&self) -> u64;
    fn uid(&self) -> u32;
    fn gid(&self) -> u32;
    fn rdev(&self) -> u64;
    fn size(&self) -> u64;
    fn atime(&self) -> i64;
    fn atime_nsec(&self) -> i64;
    fn mtime(&self) -> i64;
    fn mtime_nsec(&self) -> i64;
    fn ctime(&self) -> i64;
    fn ctime_nsec(&self) -> i64;
    fn blksize(&self) -> u64;
    fn blocks(&self) -> u64;
}

impl MetadataExt for Metadata {
    fn dev(&self) -> u64 {
        self.stat().dev()
    }
    fn ino(&self) -> u64 {
        self.stat().st_ino
    }
    fn mode(&self) -> u32 {
        self.stat().mode()
    }
    fn nlink(&self) -> u64 {
        self.stat().nlink()
    }
    fn uid(&self) -> u32 {
        self.stat().st_uid
    }
    fn gid(&self) -> u32 {
        self.stat().st_gid
    }
    fn rdev(&self) -> u64 {
        self.stat().rdev()
    }
    fn size(&self) -> u64 {
        self.stat().st_size as u64
    }
    fn atime(&self) -> i64 {
        self.stat().st_atime
    }
    fn atime_nsec(&self) -> i64 {
        self.stat().st_atime_nsec
    }
    fn mtime(&self) -> i64 {
        self.stat().st_mtime
    }
    fn mtime_nsec(&self) -> i64 {
        self.stat().st_mtime_nsec
    }
    fn ctime(&self) -> i64 {
        self.stat().st_ctime
    }
    fn ctime_nsec(&self) -> i64 {
        self.stat().st_ctime_nsec
    }
    fn blksize(&self) -> u64 {
        self.stat().blksize()
    }
    fn blocks(&self) -> u64 {
        self.stat().st_blocks as u64
    }
}

pub trait FileTypeExt {
    fn is_block_device(&self) -> bool;
    fn is_char_device(&self) -> bool;
    fn is_fifo(&self) -> bool;
    fn is_socket(&self) -> bool;
}

impl FileTypeExt for FileType {
    fn is_block_device(&self) -> bool {
        self.kind_block_device()
    }
    fn is_char_device(&self) -> bool {
        self.kind_char_device()
    }
    fn is_fifo(&self) -> bool {
        self.kind_fifo()
    }
    fn is_socket(&self) -> bool {
        self.kind_socket()
    }
}

pub trait DirEntryExt {
    fn ino(&self) -> u64;
}

impl DirEntryExt for DirEntry {
    fn ino(&self) -> u64 {
        match self.metadata() {
            Ok(m) => MetadataExt::ino(&m),
            Err(_) => 0,
        }
    }
}

pub fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    fs::symlink_raw(original.as_ref(), link.as_ref())
}
