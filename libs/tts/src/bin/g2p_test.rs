//! Show what the phonemizer does with real sentences.
//!
//!     cargo run --bin g2p_test -- "Escape the Gummer!"

use makepad_tts::g2p;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `--ids "text"` prints just the token ids, for the ONNX reference harness.
    if args.first().map(String::as_str) == Some("--ids") {
        let text = args[1..].join(" ");
        let ids: Vec<String> = g2p::tokens(&text).iter().map(u16::to_string).collect();
        println!("{}", ids.join(","));
        return;
    }
    if args.first().map(String::as_str) == Some("--ipa") {
        println!("{}", g2p::phonemize(&args[1..].join(" ")));
        return;
    }

    let samples: Vec<String> = if args.is_empty() {
        [
            "Hi! I make games with you.",
            "I made the player jump higher!",
            "Escape the Gummer, a squishy purple blob.",
            "You scored 42 points.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    } else {
        vec![args.join(" ")]
    };

    for text in &samples {
        let phonemes = g2p::phonemize(text);
        let ids = g2p::tokens(text);
        println!("{text}");
        println!("  ipa    : {phonemes}");
        println!("  tokens : {} ids {:?}...", ids.len(), &ids[..ids.len().min(10)]);

        // Every symbol must map, or the model sees a hole where a sound should be.
        let unmapped: Vec<char> = phonemes
            .chars()
            .filter(|c| g2p::vocab::token(*c).is_none())
            .collect();
        if unmapped.is_empty() {
            println!("  vocab  : all symbols map");
        } else {
            println!("  vocab  : UNMAPPED {unmapped:?}");
        }

        // Which words missed both the lexicon and the morphology rules?
        let guessed: Vec<&str> = text
            .split(|c: char| !c.is_alphabetic() && c != '\'')
            .filter(|w| {
                !w.is_empty()
                    && g2p::lexicon::lookup(w).is_none()
                    && g2p::morph::inflected(w).is_none()
            })
            .collect();
        if !guessed.is_empty() {
            println!("  guessed: {guessed:?} (letter-to-sound)");
        }
        println!();
    }
}
