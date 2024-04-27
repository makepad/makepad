#[derive(Clone, Debug)]
pub struct Url {
    /**
     * The scheme part of the URL
     */
    pub scheme: Option<String>,
    /**
     A string containing the domain (that is the hostname) followed by (if a port was specified) a
     * ':' and the port of the URL.
     */
    pub host: String,
    /**
     * A string containing the domain of the URL.
     */
    pub hostname: String,
    /**
     * The port number of the URL if available.
     */
    pub port: Option<u16>,
    /**
     * A string containing an initial '/' followed by the path of the URL. Including query string
     * and fragment for now
     */
    pub pathname: String,
    /**
     * A boolean to set the connection as secured.
     */
    pub secure: bool
}

#[derive(Debug)]
pub enum UrlParseError {
    InvalidScheme,
    InvalidHost
}

impl Url {
    pub fn parse_string(url: String) -> Result<Url, UrlParseError> {
        Url::parse(url.as_str())
    }

    pub fn parse(url: &str) -> Result<Url, UrlParseError> {
        let (scheme, rest) = Url::scheme(url).unwrap();
        let (hostname, rest) = Url::hostname(rest).unwrap();
        let (host, port) = Url::port(scheme, &hostname);
        let pathname = ("/".to_owned() + rest).to_string();
        let secure= match scheme { Some("https") | Some("wss") => true, _ => false };
        Ok(Url {
            scheme: scheme.map(|s| s.to_string()),
            host,
            hostname,
            port,
            pathname,
            secure
        })
    }

    pub fn scheme(input: &str) -> Result<(Option<&str>, &str), UrlParseError> {
        match input.split_once("://") {
            Some((scheme, rest)) => Ok((Some(scheme), rest)),
            _ => Err(UrlParseError::InvalidScheme)
        }
    }

    pub fn hostname(input: &str) -> Result<(String, &str), UrlParseError> {
        match input.split_once("/") {
            Some((hostname, rest)) => Ok((hostname.to_string(), rest)),
            _ => Err(UrlParseError::InvalidHost)
        }
    }

    pub fn port<'a>(scheme: Option<&str>, hostname: &String) -> (String, Option<u16>) {
        match hostname.split_once(":") {
            Some((host, port)) => {
                match port.parse() {
                    Ok(port) => (host.to_string(), Some(port)),
                    _ => (host.to_string(), None)
                }
            }
            _ => (hostname.to_owned(), Url::default_port(scheme))
        }
    }

    pub fn default_port(scheme: Option<&str>) -> Option<u16> {
        match scheme.unwrap() {
            "http" | "ws" => Some(80),
            "https" | "wss" => Some(443),
            "ftp" => Some(21),
            _ => None,
        }
    }
}

impl From<&str> for Url {
    fn from(str: &str) -> Self {
        Url::parse(str).unwrap()
    }
}

impl From<String> for Url {
    fn from(str: String) -> Self {
        Url::parse(str.as_str()).unwrap()
    }
}


#[cfg(test)]
mod tests {
    use crate::Url;

    #[test]
    fn it_works() {
        let url = Url::parse("https://example.org/").unwrap();
        assert_eq!(url.scheme, Some("https".to_string()));
        assert_eq!(url.hostname, "example.org".to_string());
        assert_eq!(url.port, Some(443));
        assert_eq!(url.pathname, "/".to_string());
        assert_eq!(url.secure, true);
    }

    #[test]
    fn it_works_with_port() {
        let url = Url::parse("http://example.org:8080/").unwrap();
        assert_eq!(url.scheme, Some("http".to_string()));
        assert_eq!(url.host, "example.org".to_string());
        assert_eq!(url.hostname, "example.org:8080".to_string());
        assert_eq!(url.port, Some(8080));
        assert_eq!(url.pathname, "/".to_string());
        assert_eq!(url.secure, false);
    }

    #[test]
    fn it_works_with_pathname() {
        let url = Url::parse("https://example.org/a/long/path").unwrap();
        assert_eq!(url.scheme, Some("https".to_string()));
        assert_eq!(url.hostname, "example.org".to_string());
        assert_eq!(url.pathname, "/a/long/path".to_string());
        assert_eq!(url.secure, true);
    }

    #[test]
    fn if_converts_from_str() {
        let url: Url = "https://example.org/".into();
        assert_eq!(url.hostname, "example.org".to_string());
    }
}