use {
    crate::unix_str::{UnixString, UnixStr},
    makepad_micro_serde::{DeBin, DeBinErr, SerBin},
};

#[derive(Clone, DeBin, Debug, SerBin)]
pub struct UnixPathBuf {
    string: UnixString
}

#[derive(Debug)]
pub struct UnixPath {
    string: UnixStr
}