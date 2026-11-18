//! Starting a party: the box's HTTPS identity, the party's store, the hub
//! worker and the service guests reach. The party server and the big screen
//! start it the same way.

use crate::api::{Api, Party};
use crate::hub::BoxHub;
use crate::server::{Handler, Server};
use crate::{cert, code, worker};
use makepad_network::tls::{SubjectName, TlsServer};
use std::net::IpAddr;
use std::sync::Arc;

/// The box's TLS service and the names its certificate carries.
pub struct Identity {
    pub tls: TlsServer,
    pub lan: Option<IpAddr>,
    pub names: Vec<SubjectName>,
    pub fingerprint: String,
}

/// The box's identity on its LAN address and `extra` names (a public
/// address the host gives, host names).
pub fn identity(extra: &[String]) -> Result<Identity, String> {
    let lan = cert::primary_lan_ip();
    let names = cert::names(lan, extra);
    let identity = cert::load(&names).map_err(|e| format!("TLS identity: {e}"))?;
    let fingerprint = identity.fingerprint_hex();
    let tls = TlsServer::new(identity).map_err(|e| format!("TLS server: {e}"))?;
    Ok(Identity { tls, lan, names, fingerprint })
}

/// The addresses guests open: the LAN address first, then `extra`.
pub fn urls(lan: Option<IpAddr>, extra: &[String], port: u16, code: &str, page: &str) -> Vec<String> {
    lan.map(|ip| ip.to_string())
        .into_iter()
        .chain(extra.iter().cloned())
        .map(|host| match host.parse::<IpAddr>() {
            Ok(IpAddr::V6(ip)) => format!("https://[{ip}]:{port}/p/{code}{page}"),
            _ => format!("https://{host}:{port}/p/{code}{page}"),
        })
        .collect()
}

/// What a party is to the process that runs it.
pub struct Running {
    pub party: Arc<Party>,
    pub server: Server,
    pub code: String,
    /// Where guests join, the LAN address first.
    pub urls: Vec<String>,
    pub names: Vec<SubjectName>,
    pub fingerprint: String,
}

/// Starts the party `code` (a new one when `None`) on `port`, redrawing with
/// the hub at `hub` (the box's own when `None`).
pub fn party(port: u16, code: Option<String>, extra: &[String], hub: Option<&str>) -> Result<Running, String> {
    let code = code.unwrap_or_else(code::new_code);
    if !code::valid(&code) {
        return Err("a party code is 6 to 32 of 23456789abcdefghjkmnpqrstuvwxyz".into());
    }
    let id = identity(extra)?;
    // Kept sources of this party, and only those, go here.
    let kept = makepad_network::fleet::makepad_home().join("party").join("kept").join(&code);
    let budget = Party::default_budget();
    eprintln!("[party] guests' pictures may hold up to {} MB of memory, {} MB from any one address", budget.total >> 20, budget.per_address >> 20);
    let party = Arc::new(Party::new(kept, budget).with_settings(crate::settings::Settings::load(crate::settings::Settings::file())));
    match BoxHub::locate(hub) {
        Ok(box_hub) => {
            eprintln!("[party] redrawing with the hub at {}", box_hub.base);
            worker::start(party.clone(), box_hub).map_err(|e| format!("hub worker: {e}"))?;
            worker::keep_warm(party.clone(), BoxHub::locate(hub)?).map_err(|e| format!("keeping the model warm: {e}"))?;
            // The live call has its own client of the same hub.
            party.live.redraw_with(Box::new(crate::live::HubRedrawer { hub: BoxHub::locate(hub)?, settings: party.settings.clone() }));
        }
        Err(e) => eprintln!("[party] {e}: takes wait until the party runs where a hub is"),
    }
    let api = match BoxHub::locate(hub) {
        Ok(box_hub) => Api::new(party.clone()).with_hub(box_hub),
        Err(_) => Api::new(party.clone()),
    };
    let handler: Arc<dyn Handler> = Arc::new(api);
    let server = Server::start(([0, 0, 0, 0], port).into(), id.tls, code.clone(), handler)
        .map_err(|e| format!("listen on port {port}: {e}"))?;
    // Live calls come to the same port number over UDP, on the LAN address.
    #[cfg(target_os = "linux")]
    match id.lan {
        Some(lan) => match crate::live::serve(&party, ([0, 0, 0, 0], server.port).into(), &[lan]) {
            Ok(()) => eprintln!("[party] live calls on UDP port {}", server.port),
            Err(e) => eprintln!("[party] live is off: UDP port {}: {e}", server.port),
        },
        None => eprintln!("[party] live is off: no LAN address to answer calls on"),
    }
    let urls = urls(id.lan, extra, server.port, &code, "");
    if let Some(first) = urls.first() {
        let _ = party.join.set(first.clone());
    }
    Ok(Running { party, server, code, urls, names: id.names, fingerprint: id.fingerprint })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_addresses() {
        let lan: IpAddr = "192.0.2.10".parse().unwrap();
        let got = urls(Some(lan), &["2001:db8::4".into(), "party.example".into()], 8443, "abcdefgh", "");
        assert_eq!(
            got,
            vec![
                "https://192.0.2.10:8443/p/abcdefgh",
                "https://[2001:db8::4]:8443/p/abcdefgh",
                "https://party.example:8443/p/abcdefgh",
            ],
            "the LAN address first, IPv6 in brackets, no trailing slash (an empty path segment is refused)"
        );
    }
}
