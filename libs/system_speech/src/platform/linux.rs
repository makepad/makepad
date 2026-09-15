//! Linux: no system speech recognizer exists, so STT is always unavailable.
//! TTS shells out to the `espeak-ng` command-line synthesizer (falling back
//! to the older `espeak` binary name) via `std::process::Command` — no
//! linking, no bundled model.

use crate::{
    bcp47, ListenHandle, SpeechAudio, SpeechError, SttCapabilities, SttEvent, SttOptions,
    Transcript, TtsOptions, Voice, VoiceGender,
};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const STT_ENGINE: &str = "none";
pub(crate) const TTS_ENGINE: &str = "espeak-ng";

const SYNTHESIZE_TIMEOUT: Duration = Duration::from_secs(60);

// ------------------------------------------------------------------- probe

/// Which binary name works, probed once and cached: `espeak-ng` is tried
/// first, `espeak` (older distros) second. `None` means neither runs.
fn espeak_binary() -> Option<&'static str> {
    static BINARY: OnceLock<Option<&'static str>> = OnceLock::new();
    *BINARY.get_or_init(|| {
        for bin in ["espeak-ng", "espeak"] {
            let ok = Command::new(bin)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if ok {
                return Some(bin);
            }
        }
        None
    })
}

// --------------------------------------------------------------------- STT

fn stt_unavailable() -> SpeechError {
    SpeechError::Unavailable("linux has no system speech recognizer".to_string())
}

pub(crate) fn stt_available() -> bool {
    false
}

pub(crate) fn stt_capabilities() -> SttCapabilities {
    SttCapabilities::default()
}

pub(crate) fn stt_prepare(_language: &str) -> Result<(), SpeechError> {
    Err(stt_unavailable())
}

pub(crate) fn stt_transcribe(_samples_16k: &[f32], _options: &SttOptions) -> Result<Transcript, SpeechError> {
    Err(stt_unavailable())
}

pub(crate) fn stt_listen(_options: &SttOptions, _sink: Sender<SttEvent>) -> Result<ListenHandle, SpeechError> {
    Err(stt_unavailable())
}

// --------------------------------------------------------------------- TTS

pub(crate) fn tts_available() -> bool {
    espeak_binary().is_some()
}

/// Parse the `Pty Language Age/Gender VoiceName File Other Languages` table
/// printed by `espeak-ng --voices` / `espeak --voices`. The header line is
/// skipped by position (it never has a usable data shape anyway).
fn parse_voices_table(output: &str) -> Vec<Voice> {
    let mut voices = Vec::new();
    for (i, line) in output.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let language = espeak_language_to_bcp47(fields[1]);
        let gender = match fields[2].chars().last() {
            Some('M') => VoiceGender::Male,
            Some('F') => VoiceGender::Female,
            _ => VoiceGender::Unknown,
        };
        let name = fields[3].to_string();
        voices.push(Voice { id: name.clone(), name, language, gender, offline: true });
    }
    voices
}

/// `"en-gb"` -> `"en-GB"`: keep the language part as espeak wrote it,
/// uppercase the region/variant suffix.
fn espeak_language_to_bcp47(language_col: &str) -> String {
    match language_col.split_once('-') {
        Some((lang, region)) => format!("{lang}-{}", region.to_ascii_uppercase()),
        None => language_col.to_string(),
    }
}

pub(crate) fn tts_voices() -> Vec<Voice> {
    let Some(bin) = espeak_binary() else { return Vec::new() };
    match Command::new(bin).arg("--voices").output() {
        Ok(output) if output.status.success() => {
            parse_voices_table(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// espeak `-s` words-per-minute: 175 is its own default at `rate == 1.0`.
fn wpm_from_rate(rate: f32) -> u32 {
    (175.0 * rate).clamp(80.0, 450.0).round() as u32
}

/// espeak `-p` pitch, 0..99: 50 is its own default at `pitch == 1.0`.
fn pitch_from_pitch(pitch: f32) -> u32 {
    (50.0 * pitch).clamp(0.0, 99.0).round() as u32
}

/// Run `<bin> --stdout -v <voice> -s <wpm> -p <pitch>`, feeding `text` on
/// stdin (never argv — it can be long and start with `-`). Reader threads
/// drain stdout/stderr concurrently so a large WAV can't deadlock the pipe;
/// the main thread only polls `try_wait`, killing the child past the cap.
fn run_espeak(bin: &str, voice: &str, wpm: u32, pitch: u32, text: &str) -> Result<Vec<u8>, SpeechError> {
    let mut child = Command::new(bin)
        .args(["--stdout", "-v", voice, "-s", &wpm.to_string(), "-p", &pitch.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SpeechError::Backend(format!("failed to spawn {bin}: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
        // dropped here, closing the pipe so espeak sees EOF on stdin
    }

    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let stdout_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = stdout_tx.send(buf);
    });
    let (stderr_tx, stderr_rx) = mpsc::channel();
    let stderr_reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        let _ = stderr_tx.send(buf);
    });

    let deadline = Instant::now() + SYNTHESIZE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SpeechError::Timeout);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(SpeechError::Backend(format!("wait failed: {e}"))),
        }
    };

    let stdout_bytes = stdout_reader.join().ok().and_then(|_| stdout_rx.recv().ok()).unwrap_or_default();
    let stderr_text = stderr_reader.join().ok().and_then(|_| stderr_rx.recv().ok()).unwrap_or_default();

    if !status.success() {
        return Err(SpeechError::Backend(stderr_text.trim().to_string()));
    }
    Ok(stdout_bytes)
}

/// Some espeak-ng builds write a `data` chunk size of `0` (or `0xFFFFFFFF`)
/// to `--stdout` since they don't know the final length up front. Patch it
/// to "rest of file" before handing the bytes to `wav::decode`, which
/// otherwise reads a zero-length chunk and reports no samples.
fn patch_wav_data_size(bytes: &mut [u8]) {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return;
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]);
        let body_start = pos + 8;
        if &id == b"data" {
            if size == 0 || size == 0xFFFF_FFFF {
                let actual = (bytes.len() - body_start) as u32;
                bytes[pos + 4..pos + 8].copy_from_slice(&actual.to_le_bytes());
            }
            return;
        }
        let body_end = body_start.saturating_add(size as usize).min(bytes.len());
        pos = body_end + (size as usize & 1);
    }
}

fn decode_espeak_wav(mut bytes: Vec<u8>) -> Result<SpeechAudio, SpeechError> {
    patch_wav_data_size(&mut bytes);
    let audio = crate::wav::decode(&bytes).map_err(SpeechError::Backend)?;
    if audio.samples.is_empty() {
        return Err(SpeechError::Empty);
    }
    Ok(audio)
}

pub(crate) fn tts_synthesize(text: &str, options: &TtsOptions) -> Result<SpeechAudio, SpeechError> {
    let bin = espeak_binary()
        .ok_or_else(|| SpeechError::Unavailable("espeak-ng (or espeak) is not installed".to_string()))?;
    let wpm = wpm_from_rate(options.rate);
    let pitch = pitch_from_pitch(options.pitch);

    // options.voice wins outright; otherwise derive an espeak voice name
    // from the language, and if espeak rejects that (unknown region), fall
    // back to just the bare language part.
    let (voice, derived) = match options.voice.as_deref() {
        Some(v) if !v.is_empty() => (v.to_string(), false),
        _ => (bcp47(&options.language).to_ascii_lowercase(), true),
    };

    match run_espeak(bin, &voice, wpm, pitch, text) {
        Ok(wav) => decode_espeak_wav(wav),
        Err(_) if derived && voice.contains('-') => {
            let lang_only = voice.split('-').next().unwrap_or(&voice).to_string();
            let wav = run_espeak(bin, &lang_only, wpm, pitch, text)?;
            decode_espeak_wav(wav)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_VOICES_TABLE: &str = "\
Pty Language       Age/Gender VoiceName          File                 Other Languages
 5  en-gb              M  english              en/en-gb
 5  en-us              F  english-us           en/en-us
 5  fr                5M  french               europe/fr
 5  de                 -  german               de
";

    #[test]
    fn parses_the_voices_table() {
        let voices = parse_voices_table(SAMPLE_VOICES_TABLE);
        assert_eq!(voices.len(), 4);

        assert_eq!(voices[0].id, "english");
        assert_eq!(voices[0].name, "english");
        assert_eq!(voices[0].language, "en-GB");
        assert_eq!(voices[0].gender, VoiceGender::Male);
        assert!(voices[0].offline);

        assert_eq!(voices[1].language, "en-US");
        assert_eq!(voices[1].gender, VoiceGender::Female);

        // "5M" (age + gender) still resolves to Male via the trailing letter.
        assert_eq!(voices[2].gender, VoiceGender::Male);

        // "-" (no gender given) resolves to Unknown, and a bare language
        // code with no region passes through unchanged.
        assert_eq!(voices[3].gender, VoiceGender::Unknown);
        assert_eq!(voices[3].language, "de");
    }

    #[test]
    fn empty_or_header_only_output_yields_no_voices() {
        assert!(parse_voices_table("").is_empty());
        assert!(parse_voices_table("Pty Language Age/Gender VoiceName File Other Languages\n").is_empty());
    }

    #[test]
    fn wpm_from_rate_clamps_to_espeak_range() {
        assert_eq!(wpm_from_rate(1.0), 175);
        assert_eq!(wpm_from_rate(0.0), 80);
        assert_eq!(wpm_from_rate(0.1), 80);
        assert_eq!(wpm_from_rate(10.0), 450);
        assert_eq!(wpm_from_rate(2.0), 350);
    }

    #[test]
    fn pitch_from_pitch_clamps_to_espeak_range() {
        assert_eq!(pitch_from_pitch(1.0), 50);
        assert_eq!(pitch_from_pitch(0.0), 0);
        assert_eq!(pitch_from_pitch(-5.0), 0);
        assert_eq!(pitch_from_pitch(5.0), 99);
    }

    #[test]
    fn patches_a_zero_size_data_chunk_to_rest_of_file() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes()); // riff size: irrelevant to decode()
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&22_050u32.to_le_bytes());
        bytes.extend_from_slice(&(22_050u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&0u32.to_le_bytes()); // the espeak bug: size 0
        bytes.extend_from_slice(&1i16.to_le_bytes());
        bytes.extend_from_slice(&2i16.to_le_bytes());
        bytes.extend_from_slice(&3i16.to_le_bytes());

        assert_eq!(u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]), 0);
        // Unpatched, wav::decode sees a 0-length data chunk and no samples.
        assert_eq!(crate::wav::decode(&bytes).unwrap().samples.len(), 0);

        patch_wav_data_size(&mut bytes);

        assert_eq!(u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]), 6);
        let decoded = crate::wav::decode(&bytes).unwrap();
        assert_eq!(decoded.samples.len(), 3);
        assert_eq!(decoded.sample_rate, 22_050);
    }

    #[test]
    fn patch_leaves_a_correctly_sized_data_chunk_alone() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&38u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&22_050u32.to_le_bytes());
        bytes.extend_from_slice(&(22_050u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&1i16.to_le_bytes());

        patch_wav_data_size(&mut bytes);
        assert_eq!(u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]), 2);
    }
}
