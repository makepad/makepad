//! The attention gate a spoken turn can ask for (`"filter"` in its context frame): before the chat,
//! a small model judges whether the speech is meant for the assistant at all. An open microphone
//! hears television, people talking to each other, fragments and the assistant's own voice. The
//! gate answers one line: `SEND: <instruction>` (the speech, cleaned into one instruction: recognition
//! errors fixed, filler and the wake word dropped, references resolved from the recent dialog) or
//! `SKIP: <short reason>`. A reply it can't read fails open: dropping a real request costs more
//! than answering chatter.
//!
//! Ported from `makepad_converse`'s `QwenFilter` (its prompt and verdict line); here it runs as a
//! chat job through the hub's router, like the turn's own chat. Speech that starts with one of
//! the caller's wake names ([`wake`]) needs no judging and skips it.

use crate::chat_wire::ChatRole;
use super::TalkMessage;

/// The gate's model unless the frame names another: small and fast, as the handheld's old bridge
/// ran it.
pub const MODEL: &str = "qwen3.5-4b";
/// One verdict line, never an essay.
pub const MAX_TOKENS: u32 = 96;
/// The recent dialog the gate sees, in messages.
const RECENT: usize = 6;

pub enum Verdict {
    /// Meant for the assistant, as this instruction.
    Send(String),
    /// Not meant for the assistant, with the gate's short reason.
    Skip(String),
    /// No verdict line: the speech goes on as heard.
    Unread,
}

/// The gate's instructions, with the caller's line about its application.
pub fn system(app_context: &str) -> String {
    let mut out = String::from(
        "You are the attention gate between a room microphone and the AI assistant inside this application. ",
    );
    if !app_context.trim().is_empty() {
        out.push_str(app_context.trim());
        out.push(' ');
    }
    out.push_str(
        "Every transcribed utterance the microphone hears is shown to you, with recent dialog for \
         context. Much of it is NOT for the assistant: people talking to each other or to themselves, \
         background speech, fragments, or the assistant's own spoken replies leaking back into the \
         microphone.\n\
         \n\
         Answer with EXACTLY one line:\n\
         SEND: <instruction> — the utterance is meant for the assistant. Rewrite it as one clear, \
         self-contained instruction: fix obvious speech-recognition errors, drop filler words, \
         resolve references like \"it\" from the dialog, keep the user's language and intent.\n\
         SKIP: <short reason> — not meant for the assistant.\n\
         \n\
         Rules:\n\
         - Hesitations and unfinished fragments: SKIP (the rest is still coming).\n\
         - Speech addressed to another person in the room: SKIP.\n\
         - The assistant's own reply echoed back by the microphone: SKIP.\n\
         - Follow-ups that continue the current task are for the assistant even without an address \
         word (\"now make it red\").\n\
         \n\
         Examples:\n\
         Heard: \"uh can you um can you make the castle way bigger\" -> SEND: Make the castle much bigger.\n\
         Heard: \"mom says dinner is in five minutes\" -> SKIP: talking to someone else\n\
         Heard: \"okay I added a red dragon next to the tower\" -> SKIP: assistant's own reply echoed\n\
         Heard: \"and put a uh\" -> SKIP: unfinished fragment\n",
    );
    out
}

/// The gate's question: the recent dialog (the conversation's last user and assistant texts) and
/// what was heard.
pub fn input(heard: &str, messages: &[TalkMessage]) -> String {
    let recent: Vec<&TalkMessage> = messages.iter()
        .filter(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant) && !m.text.trim().is_empty())
        .collect();
    let mut out = String::new();
    if recent.is_empty() {
        out.push_str("Recent dialog: (none)\n");
    } else {
        out.push_str("Recent dialog:\n");
        for m in &recent[recent.len().saturating_sub(RECENT)..] {
            out.push_str(if m.role == ChatRole::User { "User: " } else { "Assistant: " });
            out.push_str(&m.text.replace(['\r', '\n'], " "));
            out.push('\n');
        }
    }
    out.push_str("\nHeard: \"");
    out.push_str(heard.trim());
    out.push('"');
    out
}

/// Speech addressed by a wake name: its first words are one of `names`, set off as an address is
/// (a comma or other pause mark after them), and spelt close to it: within two edits in five of
/// the name's letters (rounded up), and within one edit of its consonant skeleton ([`skeleton`]).
/// That keeps the misheard name ("Little Barks, who ..." for "little box") and turns away other
/// names said the same way ("Little Anna, come here"). Recognition misspells short names often,
/// and a small gate model reads an odd name as speech to someone else; the pause mark keeps a
/// sentence that merely starts with the word ("computer science is hard") with the gate. The rest
/// of the speech, when there is any, is the instruction.
pub fn wake(heard: &str, names: &[String]) -> Option<String> {
    let words: Vec<(usize, &str)> = heard.split_whitespace()
        .map(|w| (w.as_ptr() as usize - heard.as_ptr() as usize + w.len(), w))
        .collect();
    let mut best: Option<(usize, String)> = None;
    for name in names {
        let count = name.split_whitespace().filter(|w| !letters(w).is_empty()).count();
        let name: String = name.split_whitespace().map(letters).collect();
        if name.is_empty() { continue; }
        let allowed = (name.chars().count() * 2).div_ceil(5);
        // A name can be heard as one word more or fewer ("little box" as "littlebox").
        for k in [count, count + 1, count.saturating_sub(1)] {
            if k == 0 { continue; }
            let Some(&(end, last)) = words.get(k - 1) else { continue };
            if !last.ends_with([',', '.', '!', '?', ':', ';', '\u{2014}']) { continue; }
            let said: String = words[..k].iter().map(|(_, w)| letters(w)).collect();
            let distance = spelling_distance(&said, &name);
            if distance > allowed || spelling_distance(&skeleton(&said), &skeleton(&name)) > 1 { continue; }
            if best.as_ref().is_some_and(|(d, _)| *d <= distance) { continue; }
            let rest = heard[end..]
                .trim_start_matches(|c: char| c.is_whitespace() || ",.!?:;-\u{2014}\"'".contains(c))
                .trim_end_matches(|c: char| c.is_whitespace() || c == ',');
            if rest.chars().any(char::is_alphanumeric) {
                best = Some((distance, rest.to_string()));
            }
        }
    }
    best.map(|(_, rest)| rest)
}

/// A word's letters and digits, lowercase.
fn letters(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The consonants as they sound, roughly: vowels, h, w and y left out, c and q as k, x as ks, z
/// as s, a doubled one once ("barks" -> brks, "box" -> bks, "computer" -> kmptr).
fn skeleton(word: &str) -> String {
    let mut out = String::new();
    for c in word.chars() {
        let sounds: &[char] = match c {
            'a' | 'e' | 'i' | 'o' | 'u' | 'y' | 'h' | 'w' => &[],
            'c' | 'q' => &['k'],
            'x' => &['k', 's'],
            'z' => &['s'],
            _ => std::slice::from_ref(&c),
        };
        for &s in sounds {
            if !out.ends_with(s) { out.push(s); }
        }
    }
    out
}

/// Edits (insert, delete, replace a character) from one text to the other.
fn spelling_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1).min(row[j] + 1).min(diagonal + usize::from(ca != cb));
            diagonal = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// The gate's reply as a verdict: its first `SEND:` or `SKIP:` line.
pub fn verdict(reply: &str) -> Verdict {
    // A thinking block, though the gate asks for none.
    let reply = reply.rsplit("</think>").next().unwrap_or(reply);
    for line in reply.lines() {
        let line = line.trim();
        let head = line.get(..5).map(str::to_ascii_uppercase);
        match head.as_deref() {
            Some("SEND:") if !line[5..].trim().is_empty() => return Verdict::Send(line[5..].trim().to_string()),
            Some("SKIP:") => return Verdict::Skip(line[5..].trim().to_string()),
            _ => {}
        }
    }
    Verdict::Unread
}
