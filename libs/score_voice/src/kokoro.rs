//! Kokoro as a source of singers (feature `kokoro`): its G2P pronounces
//! lyrics, and any Kokoro voice pack becomes a voice profile by speaking a
//! calibration text and analysing it phoneme by phoneme (Kokoro times every
//! phoneme it speaks).

use crate::analysis::{profile_from_speech, PhoneSpan};
use crate::profile::VoiceProfile;
use makepad_ai_speech::kokoro::KokoroSpeaker;

/// A word's pronunciation (Kokoro's IPA).
pub fn pronounce(word: &str) -> String {
    makepad_ai_speech::g2p::pronounce(word)
}

/// Covers every English vowel (several times each, in stressed words held
/// a little) and every consonant.
pub const CALIBRATION: &str = "Father, see the blue moon. Go, sing, say, sigh, bird. \
Cat, bed, put, but, law, boy, now, hot, sheet. \
Zoo, measure, thin, then, very, fish, jump, church, yes, we, run, long, hello, pack, tick, dog, good, bag. \
Aah, eee, ooh, oh, ay. Many rare lilies fly away. Hush, the song is sweet.";

/// Speak the calibration text with `speaker` and analyse it into a profile.
pub fn analyze_voice(speaker: &mut KokoroSpeaker, name: &str) -> Result<VoiceProfile, String> {
    // A slower delivery holds the vowels longer.
    let audio = speaker.synthesize_with_speed(CALIBRATION, 0.8).map_err(|e| format!("kokoro: {e:?}"))?;
    let phones = audio.phones.as_ref().ok_or("kokoro gave no phone timings")?;
    let spans: Vec<PhoneSpan> = phones.iter().map(|p| PhoneSpan { phone: p.phone.clone(), start: p.start, end: p.end }).collect();
    Ok(profile_from_speech(&audio.samples, audio.sample_rate as f32, &spans, name))
}

/// Load Kokoro (`model` weights, `voice` pack) and analyse the voice into a
/// profile named `name`.
pub fn analyze_pack(model: &str, voice: &str, name: &str) -> Result<VoiceProfile, String> {
    let mut speaker = KokoroSpeaker::load_with_voice(model, voice).map_err(|e| format!("kokoro: {e:?}"))?;
    analyze_voice(&mut speaker, name)
}
