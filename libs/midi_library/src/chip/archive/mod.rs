//! The archive formats the music collections ship in, read with our own
//! code: 7z (LZMA, LZMA2, PPMd, BCJ2).

pub mod bcj2;
pub mod lzma;
pub mod ppmd;
pub mod sevenz;
