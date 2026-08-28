//! One asset's trip through the pass: turntable sheet in, model-ready image
//! and question out.
//!
//! Both hosts share this — the operator CLI and the job worker — so a
//! change to how a sheet is framed, exposed or introduced to the model
//! reaches them at the same moment. Nothing here talks to a model or to the
//! catalog; it prepares what [`crate::executor`] runs and what
//! [`crate::plan_upload`] publishes.

use crate::sheet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How a sheet is prepared for the vision tower.
#[derive(Clone, Copy, Debug)]
pub struct SheetPrep {
    /// Square edge the 4x4 sheet is downscaled to before the model.
    pub sheet_size: usize,
    /// Gamma lift on subject pixels; 1.0 disables. Kenney renders sit on a
    /// dark background and a 9B tower reads a dim subject as "black".
    pub exposure: f32,
}

impl Default for SheetPrep {
    fn default() -> Self {
        Self { sheet_size: 512, exposure: 1.8 }
    }
}

/// Decode a published turntable sheet and produce the PPM the executor
/// reads.
///
/// `person` zooms each of the 16 cells onto its subject first: a character
/// is small in its cell, and the tower's patch budget was landing on empty
/// background instead of the face. Kit pieces keep their true in-cell size,
/// because that is what the `size` line reports.
pub fn sheet_to_ppm(png: &[u8], person: bool, prep: &SheetPrep) -> Result<Vec<u8>, String> {
    let mut img = sheet::decode_png(png)?;
    if person {
        img = sheet::zoom_to_subject(&img, 4, 0.15);
    }
    // Lift BEFORE downscaling: the box filter then averages corrected
    // values rather than correcting an already-averaged shadow.
    sheet::lift_exposure(&mut img, prep.exposure);
    Ok(sheet::to_ppm(&sheet::downscale(&img, prep.sheet_size)))
}

/// The prompt asked about this asset.
pub fn prompt_for(person: bool) -> &'static str {
    if person {
        crate::PROMPT_PERSON
    } else {
        crate::PROMPT
    }
}

/// The per-job context column: what the kit calls this piece, and which
/// kit it is.
///
/// Withholding the name only makes the model guess — an upright grey slab
/// reads as a book until you know the kit calls it "wall". The images still
/// decide everything the name cannot say (colour, orientation, how it
/// connects), which is the whole point of running a vision model instead of
/// parsing file names.
pub fn context_line(alias: &str, person: bool) -> String {
    let (kit, name) = alias.rsplit_once('/').unwrap_or(("", alias));
    let kit = kit.rsplit('/').next().unwrap_or("");
    if person {
        // Person framing: the SET name is context, but the questions are
        // about the person. No construction-kit talk — a character does not
        // snap onto a grid, and the grid frame biases the answers toward
        // objects.
        let set = if kit.is_empty() {
            String::new()
        } else {
            format!(" It comes from the Kenney \"{kit}\" set.")
        };
        return format!(
            "The set calls this character \"{name}\".{set} Describe the PERSON you see; \
             trust the images over the name where they disagree."
        );
    }
    let frame = if kit.is_empty() {
        String::new()
    } else {
        format!(
            " It is a game asset from the Kenney \"{kit}\" set, a low-poly modular \
             construction kit whose pieces snap together on a grid."
        )
    };
    format!(
        "The kit calls this piece \"{name}\".{frame} Trust the images over the name \
         where they disagree."
    )
}

/// One line of the executor's jobs TSV.
pub fn job_line(id: &str, ppm_path: &std::path::Path, context: &str) -> String {
    format!("{id}\t{}\t{context}\n", ppm_path.display())
}

/// Minimal GET against the store's data plane, which the asset client does
/// not wrap. Localhost or LAN, one request per connection, bounded read.
pub fn http_get(addr: SocketAddr, path: &str, token: &str) -> Result<Vec<u8>, String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    s.set_write_timeout(Some(Duration::from_secs(30))).ok();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
         Accept: */*\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("read: {e}"))?;
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no header terminator")?;
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or("no status line")?;
    if status != 200 {
        return Err(format!("HTTP {status} for {path}"));
    }
    Ok(buf[split + 4..].to_vec())
}

/// The published turntable sheet for one alias.
pub fn thumbnail_sheet(data: SocketAddr, token: &str, alias: &str) -> Result<Vec<u8>, String> {
    http_get(data, &format!("/v1/thumbnails/alias/{alias}"), token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_names_the_kit_and_the_piece() {
        let kit = context_line("kenney/nature-kit/tree-pine", false);
        assert!(kit.contains("\"tree-pine\""), "{kit}");
        assert!(kit.contains("\"nature-kit\""), "{kit}");
        assert!(kit.contains("snap together on a grid"), "{kit}");
        // A person is never introduced as a piece that snaps onto a grid.
        let person = context_line("kenney/mini-dungeon/character-human", true);
        assert!(person.contains("\"character-human\""), "{person}");
        assert!(person.contains("\"mini-dungeon\""), "{person}");
        assert!(person.contains("Describe the PERSON"), "{person}");
        assert!(!person.contains("grid"), "{person}");
    }

    #[test]
    fn an_alias_without_a_kit_still_names_the_piece() {
        let line = context_line("tree", false);
        assert!(line.contains("\"tree\""), "{line}");
        assert!(!line.contains("Kenney"), "{line}");
    }

    #[test]
    fn a_job_line_is_one_tsv_row() {
        let line = job_line("abc", std::path::Path::new("/w/abc.ppm"), "ctx");
        assert_eq!(line, "abc\t/w/abc.ppm\tctx\n");
        assert_eq!(line.matches('\t').count(), 2);
    }
}
