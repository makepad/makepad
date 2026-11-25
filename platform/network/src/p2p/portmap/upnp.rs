//! UPnP Internet Gateway Device discovery, description and WAN connection control.

use super::{remaining, SITE};
use crate::lan;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    time::Instant,
};

const IGD: &str = "urn:schemas-upnp-org:device:InternetGatewayDevice:1";
const SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

pub(super) fn search(target: SocketAddr) -> String {
    format!("M-SEARCH * HTTP/1.1\r\nHOST: {target}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {IGD}\r\n\r\n")
}

fn status(line: &str) -> Option<u16> {
    let mut words = line.split_ascii_whitespace();
    match words.next()? {
        "HTTP/1.1" | "HTTP/1.0" => words.next()?.parse().ok(),
        _ => None,
    }
}

pub(super) fn location(packet: &[u8]) -> Option<&str> {
    let mut lines = std::str::from_utf8(packet).ok()?.lines();
    if status(lines.next()?) != Some(200) {
        return None;
    }
    lines.take_while(|line| !line.is_empty()).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case("location").then(|| value.trim())
    })
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Preserve the HTTP authority for Host. Connect to the advertising device itself:
/// an SSDP reply must not redirect a mapping request to another machine. A router
/// that advertises its own DNS name needs no blocking name lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Url {
    authority: String,
    path: String,
    address: SocketAddr,
}

impl Url {
    pub(super) fn parse(value: &str, device: IpAddr) -> io::Result<Self> {
        if value.bytes().any(|b| b <= b' ' || b == 127 || b == b'\\') {
            return Err(invalid("invalid byte in router URL"));
        }
        let (scheme, rest) = value.split_once("://").ok_or_else(|| invalid("missing URL scheme"))?;
        if !scheme.eq_ignore_ascii_case("http") {
            return Err(invalid("router URL requires HTTP"));
        }
        let rest = rest.split('#').next().unwrap_or_default();
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let (host, port) = authority.rsplit_once(':').unwrap_or((authority, "80"));
        // IGD discovery and these port mappings are IPv4.
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)) {
            return Err(invalid("invalid router host"));
        }
        if host.parse::<IpAddr>().is_ok_and(|ip| ip != device) {
            return Err(invalid("router URL names a different device"));
        }
        let port = port.parse().map_err(|_| invalid("invalid HTTP port"))?;
        let suffix = &rest[end..];
        Ok(Self {
            authority: authority.into(),
            address: SocketAddr::new(device, port),
            path: if suffix.starts_with('/') { suffix.to_string() } else { format!("/{suffix}") },
        })
    }

    fn resolve(&self, reference: &str) -> io::Result<Self> {
        let reference = reference.split('#').next().unwrap_or_default();
        if reference.split(['/', '?']).next().is_some_and(|part| part.contains(':')) {
            return Self::parse(reference, self.address.ip());
        }
        if reference.starts_with("//") {
            return Self::parse(&format!("http:{reference}"), self.address.ip());
        }
        if reference.is_empty() {
            return Ok(self.clone());
        }
        let base_path = self.path.split('?').next().unwrap_or("/");
        let joined = if reference.starts_with('/') {
            reference.to_owned()
        } else if reference.starts_with('?') {
            format!("{base_path}{reference}")
        } else {
            let dir = base_path.rsplit_once('/').map_or("", |(dir, _)| dir);
            format!("{dir}/{reference}")
        };
        let (path, query) = joined.split_at(joined.find('?').unwrap_or(joined.len()));
        let mut parts = Vec::new();
        for part in path[1..].split('/') {
            match part {
                "." => (),
                ".." => {
                    parts.pop();
                }
                _ => parts.push(part),
            }
        }
        if path.ends_with("/.") || path.ends_with("/..") {
            parts.push("");
        }
        Self::parse(&format!("http://{}/{}{query}", self.authority, parts.join("/")), self.address.ip())
    }
}

// The fields we need are simple text elements. Match whole local names (including
// namespace-prefixed tags), and only pair a serviceType with its own service's URL.
fn elements<'a>(mut xml: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> {
    std::iter::from_fn(move || loop {
        let (_, after) = xml.split_once('<')?;
        if let Some(comment) = after.strip_prefix("!--") {
            xml = comment.split_once("-->")?.1;
            continue;
        }
        let (tag, tail) = after.split_once('>')?;
        xml = tail;
        let tag = tag.trim_end();
        let qualified = tag.trim_end_matches('/').split_ascii_whitespace().next().unwrap_or("");
        if qualified.starts_with('/') || qualified.rsplit(':').next() != Some(name) {
            continue;
        }
        if tag.ends_with('/') {
            return Some("");
        }
        let closing = format!("</{qualified}");
        let (at, rest) = tail.match_indices(closing.as_str()).find_map(|(at, found)| {
            tail[at + found.len()..].trim_start().strip_prefix('>').map(|rest| (at, rest))
        })?;
        let content = &tail[..at];
        xml = rest;
        return Some(content.trim());
    })
}

#[derive(Clone, Debug)]
pub(super) struct Service {
    url: Url,
    kind: &'static str,
}

fn service(xml: &str, location: &Url) -> io::Result<Service> {
    let base = elements(xml, "URLBase")
        .next()
        .map(|s| location.resolve(s))
        .transpose()?
        .unwrap_or_else(|| location.clone());
    for entry in elements(xml, "service") {
        let Some(kind) = elements(entry, "serviceType").next() else { continue };
        let Some(kind) = SERVICES.iter().copied().find(|known| *known == kind) else { continue };
        if let Some(url) = elements(entry, "controlURL").next().filter(|v| !v.is_empty()) {
            return Ok(Service { kind, url: base.resolve(&url.replace("&amp;", "&"))? });
        }
    }
    Err(invalid("no WAN connection control URL in device description"))
}

struct TimedStream {
    stream: TcpStream,
    deadline: Instant,
}

impl Read for TimedStream {
    fn read(&mut self, dest: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(remaining(self.deadline)?))?;
        self.stream.read(dest)
    }
}

impl Write for TimedStream {
    fn write(&mut self, src: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(remaining(self.deadline)?))?;
        self.stream.write(src)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut value = String::new();
    reader.read_line(&mut value)?;
    if !value.ends_with('\n') {
        return Err(invalid("incomplete HTTP line"));
    }
    value.truncate(value.trim_end_matches(['\r', '\n']).len());
    Ok(value)
}

// Stream only bytes actually received into the body. A bogus advertised size
// never becomes an allocation, and a peer trickling bytes still hits the deadline.
fn copy_bytes(reader: &mut impl Read, count: u64, body: &mut Vec<u8>) -> io::Result<()> {
    if io::copy(&mut reader.take(count), body)? != count {
        return Err(invalid("short HTTP body"));
    }
    Ok(())
}

fn response(mut reader: impl BufRead) -> io::Result<(u16, String)> {
    let code = status(&line(&mut reader)?).ok_or_else(|| invalid("invalid HTTP status"))?;
    let mut length: Option<u64> = None;
    let mut chunked = false;
    loop {
        let header = line(&mut reader)?;
        if header.is_empty() {
            break;
        }
        let (key, value) = header.split_once(':').ok_or_else(|| invalid("invalid HTTP header"))?;
        if key.eq_ignore_ascii_case("content-length") {
            let n = value.trim().parse().map_err(|_| invalid("invalid Content-Length"))?;
            if length.replace(n).is_some_and(|old| old != n) {
                return Err(invalid("conflicting Content-Length"));
            }
        } else if key.eq_ignore_ascii_case("transfer-encoding") {
            if !value.trim().eq_ignore_ascii_case("chunked") {
                return Err(invalid("unsupported transfer encoding"));
            }
            chunked = true;
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let header = line(&mut reader)?;
            let size = header.split(';').next().unwrap_or("").trim();
            let n = u64::from_str_radix(size, 16).map_err(|_| invalid("invalid HTTP chunk size"))?;
            if n == 0 {
                while !line(&mut reader)?.is_empty() {}
                break;
            }
            copy_bytes(&mut reader, n, &mut body)?;
            if !line(&mut reader)?.is_empty() {
                return Err(invalid("invalid HTTP chunk terminator"));
            }
        }
    } else if let Some(n) = length {
        copy_bytes(&mut reader, n, &mut body)?;
    } else {
        reader.read_to_end(&mut body)?;
    }
    Ok((code, String::from_utf8(body).map_err(|_| invalid("invalid XML encoding"))?))
}

fn http(url: &Url, soap: Option<(&str, &str, String)>, deadline: Instant) -> io::Result<(u16, String)> {
    let stream = lan::connect_timeout(SITE, &url.address, remaining(deadline)?)?;
    let mut stream = TimedStream { stream, deadline };
    let method = if soap.is_some() { "POST" } else { "GET" };
    let mut request =
        format!("{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", url.path, url.authority);
    if let Some((kind, action, body)) = soap {
        request.push_str(&format!("Content-Type: text/xml; charset=\"utf-8\"\r\nSOAPAction: \"{kind}#{action}\"\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    } else {
        request.push_str("\r\n");
    }
    stream.write_all(request.as_bytes())?;
    response(BufReader::new(stream))
}

pub(super) fn describe(location: Url, deadline: Instant) -> io::Result<Service> {
    let (status, xml) = http(&location, None, deadline)?;
    if status != 200 {
        return Err(io::Error::other(format!("description HTTP {status}")));
    }
    service(&xml, &location)
}

#[derive(Debug)]
pub(super) enum SoapError {
    Fault(u32),
    Io(io::Error),
}

impl From<io::Error> for SoapError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl std::fmt::Display for SoapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fault(n) => write!(f, "UPnP fault {n}"),
            Self::Io(err) => err.fmt(f),
        }
    }
}

impl Service {
    fn invoke(&self, action: &str, args: &str, deadline: Instant) -> Result<String, SoapError> {
        let kind = self.kind;
        let body = format!("<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{kind}\">{args}</u:{action}></s:Body></s:Envelope>");
        let (status, reply) = http(&self.url, Some((kind, action, body)), deadline)?;
        if let Some(n) = elements(&reply, "errorCode").next() {
            return Err(SoapError::Fault(n.parse().map_err(|_| invalid("invalid SOAP errorCode"))?));
        }
        if status != 200 {
            return Err(io::Error::other(format!("{action} HTTP {status}")).into());
        }
        let response = elements(&reply, &format!("{action}Response")).next().map(str::to_owned);
        response.ok_or_else(|| invalid("missing SOAP action response").into())
    }

    pub(super) fn external_ip(&self, deadline: Instant) -> Result<Ipv4Addr, SoapError> {
        let reply = self.invoke("GetExternalIPAddress", "", deadline)?;
        let ip = elements(&reply, "NewExternalIPAddress")
            .next()
            .and_then(|s| s.parse::<Ipv4Addr>().ok())
            .filter(|ip| !ip.is_unspecified())
            .ok_or_else(|| invalid("missing external IPv4 address"))?;
        Ok(ip)
    }

    pub(super) fn lease_duration(&self, requested: u32) -> u32 {
        // WANIPConnection:2 section 2.5.16.2 specifies a maximum of one week.
        if self.kind == SERVICES[0] {
            requested.min(604800)
        } else {
            requested
        }
    }

    pub(super) fn add(
        &self,
        internal: SocketAddrV4,
        external: u16,
        lifetime: u32,
        deadline: Instant,
    ) -> Result<(), SoapError> {
        let args = format!("{}<NewInternalPort>{}</NewInternalPort><NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>makepad p2p</NewPortMappingDescription><NewLeaseDuration>{lifetime}</NewLeaseDuration>",
            mapping_key(external), internal.port(), internal.ip());
        self.invoke("AddPortMapping", &args, deadline).map(drop)
    }

    pub(super) fn delete(&self, port: u16, deadline: Instant) -> Result<(), SoapError> {
        self.invoke("DeletePortMapping", &mapping_key(port), deadline).map(drop)
    }
}

fn mapping_key(port: u16) -> String {
    format!("<NewRemoteHost></NewRemoteHost><NewExternalPort>{port}</NewExternalPort><NewProtocol>UDP</NewProtocol>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_and_device_descriptions() {
        let answer = b"HTTP/1.1 200 OK\r\nlOcAtIoN: http://127.0.0.1:80/root/desc.xml\r\n\r\n";
        let loc = Url::parse(location(answer).unwrap(), Ipv4Addr::LOCALHOST.into()).unwrap();
        let xml = format!("<root><device><deviceList><device><serviceList><service><serviceType>other</serviceType><controlURL>/wrong</controlURL></service><x:service><x:serviceType> {} </x:serviceType><controlURL> ../control?x=1&amp;y=2 </controlURL></x:service></serviceList></device></deviceList></device></root>", SERVICES[0]);
        let wan = service(&xml, &loc).unwrap();
        assert_eq!(wan.lease_duration(u32::MAX), 604800, "WANIPConnection:2 maximum lease");
        assert_eq!(
            (wan.kind, wan.url.path.as_str()),
            (SERVICES[0], "/control?x=1&y=2"),
            "nested WAN service"
        );
        for (control, expected) in [("soap", "/base/soap"), ("http://127.0.0.1:8081/ppp", "/ppp")] {
            let xml = format!("<root><URLBase> http://127.0.0.1:8081/base/ </URLBase><service><controlURL>{control}</controlURL><serviceType>{}</serviceType></service></root>", SERVICES[2]);
            let wan = service(&xml, &loc).unwrap();
            assert_eq!(
                (wan.url.address.port(), wan.url.path.as_str()),
                (8081, expected),
                "absolute/base URL resolution"
            );
        }
        assert!(Url::parse("http://127.0.0.1/x\r\nHost: injected", loc.address.ip()).is_err());
    }
}
