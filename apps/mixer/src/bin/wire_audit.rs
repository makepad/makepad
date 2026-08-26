//! `cargo run -p makepad-mixer --bin wire_audit`
//!
//! Prints the complete wire audit for the mixer app, GENERATED FROM THE
//! WHITELIST ITSELF (safety::Param) so it cannot drift from the code. This
//! is the document to read before pointing the app at real hardware. It
//! opens no socket and sends nothing.

use makepad_mixer::safety::{deny_term, MeterBank, Param, SafeMsg, ValueSpec, DENY_TERMS};
use std::collections::BTreeMap;

fn mask(addr: &str) -> String {
    // Collapse index segments so 900 addresses read as ~40 patterns.
    let mut out = String::new();
    for (i, seg) in addr.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        if !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-')
            && seg.chars().any(|c| c.is_ascii_digit())
        {
            out.push_str("NN");
        } else {
            out.push_str(seg);
        }
    }
    out
}

fn spec_str(s: ValueSpec) -> String {
    match s {
        ValueSpec::Float01 => "float 0.0..=1.0 (finite; anything else refused)".into(),
        ValueSpec::Int { max } => format!("int 0..={} (anything else refused)", max),
        ValueSpec::Name => "ASCII string, max 12 chars, no control chars".into(),
    }
}

fn main() {
    let all = Param::all_constructable();
    let mut patterns: BTreeMap<String, (usize, ValueSpec)> = BTreeMap::new();
    for p in &all {
        let e = patterns
            .entry(mask(&p.addr()))
            .or_insert((0, p.spec()));
        e.0 += 1;
    }

    println!("================ MIXER WIRE AUDIT ================");
    println!();
    println!("Generated from safety::Param::all_constructable() — the closed");
    println!("enum that is the ONLY way this app can build an outgoing OSC");
    println!("address. If it is not in this list, the app cannot say it.");
    println!();
    println!("--- 1. Every parameter address family (SET takes the listed");
    println!("       argument shape; GET is the same address with no args) ---");
    let mut total = 0usize;
    for (pat, (count, spec)) in &patterns {
        println!("  {:<28} x{:<4} SET arg: {}", pat, count, spec_str(*spec));
        total += count;
    }
    println!("  = {} concrete addresses", total);
    println!();
    println!("--- 2. The only non-parameter messages ---");
    for (m, why) in [
        (SafeMsg::xinfo(), "identity query (discovery + connect header)"),
        (SafeMsg::status(), "server status query (currently unused by the UI)"),
        (SafeMsg::xremote(), "subscribe this ip:port to change pushes, 10 s lease"),
        (
            SafeMsg::meters_subscribe(MeterBank::Channels),
            "meter bank \"/meters/1\" subscribe (blob stream, self-expires)",
        ),
        (
            SafeMsg::meters_subscribe(MeterBank::Dynamics),
            "meter bank \"/meters/6\" subscribe (gain reduction)",
        ),
    ] {
        println!("  {:<28} {}", m.human(), why);
    }
    println!();
    println!("--- 3. When anything is transmitted ---");
    println!("  at startup ............. NOTHING, ever");
    println!("  SCAN (user click) ...... one /xinfo to the typed target");
    println!("  CONNECT (user click) ... /xinfo, /xremote, both /meters subscribes,");
    println!("                           then a paced GET sweep of section 1 (no args =");
    println!("                           read), fire-and-forget at 64 msgs / 15 ms (~300 ms total)");
    println!("  every 8 s connected .... /xremote renewal (10 s lease)");
    println!("  every 7 s connected .... both /meters renewals (their 200-frame/10 s");
    println!("                           streams overlap instead of gapping)");
    println!("  fader/pan/gain/thr drag  one SET, throttled to 25/s, final value on release");
    println!("  MUTE click ............. one SET of mix/on 0|1");
    println!("  view-only mode ......... the default on EVERY new connection, enforced");
    println!("                           AT THE SOCKET: while active, any packet that");
    println!("                           carries an argument (= a SET in this dialect)");
    println!("                           is refused at the transmit gate, except the");
    println!("                           /meters subscribe. Stage-1 wire = /xinfo,");
    println!("                           /xremote, /meters, and bare-address GETs. Only");
    println!("                           the app lifts it once a session is up.");
    println!("  received packets ....... can NEVER trigger a transmission. Malformed or");
    println!("                           unexpected input is dropped (wrong shape, wrong");
    println!("                           type, unknown or dangerous address, bad blob");
    println!("                           lengths) — the receive path has no send calls.");
    println!();
    println!("--- 4. The deny list (checked at the single socket write, on the");
    println!("       exact bytes about to leave, independent of everything above) ---");
    println!("  refuse any address containing: {:?}", DENY_TERMS);
    println!();
    println!("--- 5. Cross-check: no constructable address matches the deny list ---");
    let mut bad = 0;
    for p in &all {
        if let Some(t) = deny_term(&p.addr()) {
            println!("  COLLISION: {} matches {:?}", p.addr(), t);
            bad += 1;
        }
    }
    if bad == 0 {
        println!("  OK — {} addresses checked, zero collisions", total);
    }
    println!();
    println!("--- 6. What is NOT here (by construction, not by policy) ---");
    println!("  phantom power (/headamp/NN/phantom), snapshots/scenes/presets");
    println!("  (/-snap, /snap, /-libs, /-show, /load, /save), console actions");
    println!("  (/-action incl. initall), preferences (/-prefs), status writes");
    println!("  (/-stat), USB (/-usb), routing (/routing, /config/routing),");
    println!("  input repatch (insrc/rtnsrc). No enum variant renders them, no");
    println!("  UI element maps to them, and the socket gate refuses them anyway.");
    println!();
    println!("Source port: OS-assigned at bind (never fixed), so this app");
    println!("coexists with other controllers; replies and pushes address the");
    println!("same ip:port. In --fake mode the socket binds 127.0.0.1 and");
    println!("refuses any non-loopback destination at the same gate.");
}
