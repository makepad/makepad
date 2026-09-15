fn main() {
    let config = match makepad_web_server::Config::parse_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = makepad_web_server::run(config) {
        eprintln!("makepad-web-server: {error}");
        std::process::exit(1);
    }
}
