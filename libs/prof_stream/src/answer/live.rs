//! Live app state: Splash source (`src`), VM values (`vm`), the script->host call trace
//! (`verbs`), script error sites (`script errors`), the game world (`game`), field watches
//! (`writes`). The renderers give the finding first; rows go to the generic tables.

use super::{f, s, Answer};
use makepad_strict_json::Value;

/// `src <file>`: the lines as a numbered listing; without a file the bodies' table.
pub fn src(a: &mut Answer) {
    let _ = a.take1("vm");
    let head = a.take1("src");
    let lines = a.take("line");
    if let Some(h) = head {
        a.head = format!(
            "{} ({}, {} lines), lines {}..{}{}:",
            s(&h, "src"),
            s(&h, "body"),
            f(&h, "lines"),
            f(&h, "from"),
            f(&h, "to"),
            if f(&h, "also") > 0.0 { format!("; {} more bodies match, name one as B<n>", f(&h, "also")) } else { String::new() }
        );
        let mut t = String::from("```\n");
        for l in &lines {
            t.push_str(&format!("{:>5}  {}\n", f(l, "n"), s(l, "line")));
        }
        t.push_str("```");
        a.line(&t);
        a.more(format!("src {} line=<n>", s(&h, "body")));
        a.more("verbs on");
    } else {
        let n = a.lines.iter().filter(|l| l.get("body").is_some()).count();
        a.head = format!("{} script bodies in this VM, newest first; `src <file part>` lists one with line numbers.", n);
    }
}

pub fn script_errors(a: &mut Answer) {
    let top = a.take1("script_errors");
    let n = top.as_ref().map_or(0.0, |t| f(t, "script_errors"));
    if n == 0.0 {
        a.head = "No script error or host diagnostic recorded.".into();
        return;
    }
    let first = a.lines.iter().zip(a.used.iter()).find(|(l, u)| !**u && l.get("text").is_some()).map(|(l, _)| l.clone());
    a.head = match first {
        Some(l) => format!("{} error sites; most frequent: {} x{} at {}: {}", n, s(&l, "kind"), f(&l, "n"), s(&l, "at"), s(&l, "text")),
        None => format!("{} error sites", n),
    };
    a.more("src <file> line=<n>");
}

pub fn vm(a: &mut Answer) {
    let vm = a.take1("vm").map(|v| s(&v, "vm").to_string()).unwrap_or_default();
    if let Some(p) = a.take1("path") {
        a.head = format!("{} {} = {}  (from {})", vm, s(&p, "path"), s(&p, "value"), s(&p, "from"));
        return;
    }
    if let Some(sc) = a.take1("scope") {
        a.head = format!("{}: {} top-level names in {} ({}):", vm, f(&sc, "names"), s(&sc, "scope"), s(&sc, "body"));
        a.more("vm get <name>[i].field");
    }
}

pub fn verbs(a: &mut Answer) {
    let Some(t) = a.take1("trace") else { return };
    let state = s(&t, "trace").to_string();
    if t.get("verbs").is_some() && t.get("calls").is_none() {
        a.head = format!("verb trace {}.", state);
        if let Some(n) = t.get("note").and_then(Value::as_str) {
            a.line(n);
        }
        return;
    }
    a.head = format!("verb trace {}: {} calls recorded, the latest {} (oldest first):", state, f(&t, "calls"), f(&t, "shown"));
    a.more("verbs by=site");
    a.more("verbs off");
}

pub fn game(a: &mut Answer) {
    let sub = a.req.pos.first().map(String::as_str).unwrap_or("worlds");
    if sub == "worlds" {
        a.head = "live game worlds (game actors / entities answer for the largest unless world=W<n>):".into();
    } else if let Some(w) = a.take1("world") {
        a.head = format!("world {}: {} entities, {} actors.", s(&w, "world"), f(&w, "entities"), f(&w, "actors"));
    }
    a.more("game actors");
    a.more("game actor <id>");
}

pub fn writes(a: &mut Answer) {
    if let Some(w) = a.take1("watch") {
        if w.get("writer_sites").is_some() {
            a.head = format!("{} on {}: {} writes at {} writer sites (sites by writes, then the latest writes):", s(&w, "watch"), s(&w, "field"), f(&w, "writes"), f(&w, "writer_sites"));
            a.more(format!("writes {} n=50", s(&w, "watch")));
            a.more(format!("writes rm {}", s(&w, "watch")));
        } else {
            a.lines.insert(0, w);
            a.used.insert(0, false);
            a.head = "field watches:".into();
        }
    }
}
