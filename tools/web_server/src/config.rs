use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub root: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub nav_basename: PathBuf,
    pub searchdb: Option<PathBuf>,
    pub places: Option<PathBuf>,
    pub major_graph: Option<PathBuf>,
    pub chargers: Option<PathBuf>,
    pub route_workers: usize,
    pub route_queue: usize,
    pub query_workers: usize,
}

impl Config {
    pub fn parse_env() -> Result<Self, String> {
        Self::parse(std::env::args().skip(1))
    }

    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        #[cfg(target_os = "linux")]
        let mut listen: SocketAddr = "0.0.0.0:80".parse().unwrap();
        #[cfg(not(target_os = "linux"))]
        let mut listen: SocketAddr = "127.0.0.1:61234".parse().unwrap();

        let mut root = None;
        let mut data_dir = None;
        let mut nav_basename = PathBuf::from("maps/noord-holland");
        let mut searchdb = Some(PathBuf::from("maps/europe.searchdb"));
        let mut places = Some(PathBuf::from("maps/europe-places.search"));
        let mut major_graph = None;
        let mut chargers = None;
        let mut route_workers = 1usize;
        let mut route_queue = 8usize;
        let mut query_workers = 2usize;
        let mut args = args.into_iter().map(Into::into).peekable();

        while let Some(arg) = args.next() {
            let value = |name: &str, args: &mut std::iter::Peekable<_>| {
                args.next().ok_or_else(|| format!("missing value for {name}"))
            };
            match arg.as_str() {
                "--listen" => {
                    listen = value("--listen", &mut args)?
                        .parse()
                        .map_err(|_| "invalid --listen socket address".to_string())?;
                }
                "--port" => {
                    let port: u16 = value("--port", &mut args)?
                        .parse()
                        .map_err(|_| "invalid --port".to_string())?;
                    listen.set_port(port);
                }
                "--root" => root = Some(PathBuf::from(value("--root", &mut args)?)),
                "--data-dir" => {
                    data_dir = Some(PathBuf::from(value("--data-dir", &mut args)?))
                }
                "--nav-basename" => {
                    nav_basename = PathBuf::from(value("--nav-basename", &mut args)?)
                }
                "--searchdb" => {
                    searchdb = optional_path(value("--searchdb", &mut args)?)
                }
                "--places" => places = optional_path(value("--places", &mut args)?),
                "--major-graph" => {
                    major_graph = optional_path(value("--major-graph", &mut args)?)
                }
                "--chargers" => chargers = optional_path(value("--chargers", &mut args)?),
                "--route-workers" => {
                    route_workers = parse_count("--route-workers", value("--route-workers", &mut args)?, 1, 16)?
                }
                "--route-queue" => {
                    route_queue = parse_count("--route-queue", value("--route-queue", &mut args)?, 1, 8)?
                }
                "--query-workers" => {
                    query_workers = parse_count("--query-workers", value("--query-workers", &mut args)?, 1, 16)?
                }
                "-h" | "--help" => return Err(usage().to_string()),
                _ if arg.starts_with('-') => return Err(format!("unknown option {arg}\n{}", usage())),
                _ if root.is_none() => root = Some(PathBuf::from(arg)),
                _ => return Err(format!("unexpected positional argument {arg}")),
            }
        }
        let root = root.ok_or_else(|| format!("missing --root\n{}", usage()))?;
        Ok(Self {
            listen,
            root,
            data_dir,
            nav_basename,
            searchdb,
            places,
            major_graph,
            chargers,
            route_workers,
            route_queue,
            query_workers,
        })
    }
}

fn optional_path(value: String) -> Option<PathBuf> {
    (value != "off").then(|| PathBuf::from(value))
}

fn parse_count(name: &str, value: String, min: usize, max: usize) -> Result<usize, String> {
    let parsed = value.parse::<usize>().map_err(|_| format!("invalid {name}"))?;
    if !(min..=max).contains(&parsed) {
        return Err(format!("{name} must be in {min}..={max}"));
    }
    Ok(parsed)
}

pub fn usage() -> &'static str {
    "usage: makepad-web-server [ROOT] [--port PORT] [--listen ADDR] [--root ROOT] \
     [--data-dir DIR] [--nav-basename PATH] [--searchdb PATH|off] [--places PATH|off] \
     [--major-graph PATH|off] [--chargers PATH|off] [--route-workers N] \
     [--route-queue N] [--query-workers N]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_root_and_port_still_parse() {
        let config = Config::parse(["site", "--port", "9000"]).unwrap();
        assert_eq!(config.root, PathBuf::from("site"));
        assert_eq!(config.listen.port(), 9000);
        assert!(config.data_dir.is_none());
    }

    #[test]
    fn deployment_flags_parse() {
        let config = Config::parse([
            "--listen", "127.0.0.1:8080", "--root", "site", "--data-dir", "data",
            "--nav-basename", "maps/test", "--searchdb", "off", "--places", "places.search",
            "--major-graph", "major.graph", "--chargers", "off", "--route-workers", "2",
            "--route-queue", "4", "--query-workers", "3",
        ]).unwrap();
        assert_eq!(config.listen.port(), 8080);
        assert!(config.searchdb.is_none());
        assert_eq!(config.route_workers, 2);
        assert_eq!(config.route_queue, 4);
        assert_eq!(config.query_workers, 3);
        assert!(Config::parse(["site", "--route-queue", "9"]).is_err());
    }
}
