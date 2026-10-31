//! Engine requests: an app asks whoever maintains its engine's code for a change it cannot make
//! itself (the Sandbox's in-game AI needs an engine verb that does not exist). Under `makepad
//! run` a request is an `engine_request` event on the JIT's observation channel
//! (`std::relax::post_engine_request`); the node's engine relay passes it to the central agent,
//! whose answers (received / working / deployed / declined / note) come back through
//! [`replies_after`]. A rustc build has no observation channel: [`post`] says so.

/// One request; every field is plain text (the host bounds and stores it verbatim).
#[derive(Clone, Debug, Default)]
pub struct EngineRequest {
    pub game: String,
    pub summary: String,
    pub details: String,
    /// an excerpt of the conversation that led to the request
    pub context: String,
    /// where answers go inside the app (a chat session id); carried back verbatim
    pub reply_route: String,
}

/// One answer to a request.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineReply {
    pub seq: u64,
    pub id: String,
    /// posted (the request itself) | received | working | deployed | declined | note |
    /// code_update (id "": a live edit was applied to this process)
    pub status: String,
    /// the request's summary (empty on code_update)
    pub summary: String,
    pub message: String,
}

/// Whether requests can be posted in this run (under `makepad run`).
pub fn available() -> bool {
    cfg!(relax)
}

/// Posts `req`; returns its id.
pub fn post(req: &EngineRequest) -> Result<String, String> {
    #[cfg(relax)]
    {
        std::relax::post_engine_request(&std::relax::EngineRequest {
            game: &req.game,
            summary: &req.summary,
            details: &req.details,
            context: &req.context,
            reply_route: &req.reply_route,
        })
    }
    #[cfg(not(relax))]
    {
        let _ = req;
        Err("engine requests need the JIT's observation channel: run the app with `makepad run`".into())
    }
}

/// The request stream after record `after` (0: the whole history), oldest first.
pub fn replies_after(after: u64) -> Vec<EngineReply> {
    #[cfg(relax)]
    {
        std::relax::engine_replies(after).lines().filter_map(parse_reply).collect()
    }
    #[cfg(not(relax))]
    {
        let _ = after;
        Vec::new()
    }
}

/// One reply line (`{"seq":..,"id":..,"status":..,"message":..,"at_ms":..}`, written by the host).
pub fn parse_reply(line: &str) -> Option<EngineReply> {
    let seq = number(line, "seq")?;
    Some(EngineReply { seq, id: string(line, "id")?, status: string(line, "status")?, summary: string(line, "summary").unwrap_or_default(), message: string(line, "message").unwrap_or_default() })
}

fn number(line: &str, key: &str) -> Option<u64> {
    let at = line.find(&format!("\"{key}\":"))? + key.len() + 3;
    let digits: String = line[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn string(line: &str, key: &str) -> Option<String> {
    let at = line.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = line[at..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('\u{fffd}'));
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn reply_lines_parse() {
        let r = super::parse_reply("{\"seq\":12,\"id\":\"n1.42.3\",\"status\":\"working\",\"summary\":\"rope\",\"message\":\"on it: \\\"grapple\\\"\\nsoon\",\"at_ms\":5}").unwrap();
        assert_eq!((r.seq, r.id.as_str(), r.status.as_str(), r.message.as_str()), (12, "n1.42.3", "working", "on it: \"grapple\"\nsoon"));
        assert_eq!(r.summary, "rope");
        assert!(super::parse_reply("{\"x\":1}").is_none());
    }
}
