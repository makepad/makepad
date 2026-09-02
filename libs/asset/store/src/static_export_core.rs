//! Sink-independent, cooperatively driven static export output plan.

use crate::error::{io_err, ServerResult};
use std::collections::VecDeque;
use std::io::Write;

pub fn brotli_bytes(bytes: &[u8]) -> ServerResult<Vec<u8>> {
    let mut writer = brotli::CompressorWriter::new(Vec::new(), 4096, 9, 20);
    writer
        .write_all(bytes)
        .map_err(io_err("static export brotli encode"))?;
    writer
        .flush()
        .map_err(io_err("static export brotli flush"))?;
    Ok(writer.into_inner())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEntry {
    pub path: String,
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub content_encoding: Option<&'static str>,
}

pub trait ExportSink {
    fn write_entry(&mut self, entry: &ExportEntry) -> ServerResult<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportStep {
    Pending { written: u64, remaining: u64 },
    Complete { written: u64 },
}

/// A route plan independent of its eventual filesystem, download, or
/// storage-backed sink. One call writes at most one entry.
#[derive(Default)]
pub struct ExportPlan {
    entries: VecDeque<ExportEntry>,
    written: u64,
}

impl ExportPlan {
    pub fn push(&mut self, entry: ExportEntry) {
        self.entries.push_back(entry);
    }

    pub fn step(&mut self, sink: &mut dyn ExportSink) -> ServerResult<ExportStep> {
        if let Some(entry) = self.entries.front() {
            sink.write_entry(entry)?;
            self.entries.pop_front();
            self.written += 1;
        }
        if self.entries.is_empty() {
            Ok(ExportStep::Complete { written: self.written })
        } else {
            Ok(ExportStep::Pending {
                written: self.written,
                remaining: self.entries.len() as u64,
            })
        }
    }
}
