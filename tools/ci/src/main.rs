pub use makepad_widgets;
mod app;
mod cargo;
mod cargo_cache;
mod window_geometry;
mod drive;
mod machine;
mod pipeline;
mod process;
mod remote;
mod report;
mod runner;
mod smoke;
mod test_run;
mod uihub;
mod wall;
mod watch;

use std::{path::PathBuf, sync::Arc};

#[derive(Default)]
pub struct Options {
    script: Option<PathBuf>,
    once: Option<String>,
    install: bool,
    accept_license: bool,
    model: Option<String>,
    no_vision: bool,
    deep: bool,
}
impl Options {
    fn parse() -> process::Result<Self> {
        let mut result = Self::default();
        let args: Vec<_> = std::env::args().skip(1).collect();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--run" | "--once" => {
                    let script = args[i] == "--run";
                    let value = if args.get(i + 1).is_some_and(|a| !a.starts_with('-')) {
                        i += 1;
                        args[i].clone()
                    } else if script {
                        "apps/wm/ci.splash".into()
                    } else {
                        "work".into()
                    };
                    if script {
                        result.script = Some(value.into());
                    } else {
                        result.once = Some(value);
                    }
                }
                "--install" => result.install = true,
                "--accept-license" => result.accept_license = true,
                "--no-vision" => result.no_vision = true,
                "--deep" => result.deep = true,
                "--model" => {
                    i += 1;
                    result.model = Some(args.get(i).ok_or("--model needs an ID")?.clone());
                }
                "--help" | "-h" => {
                    println!("ci [--run [ci.splash] | --once [branch] | --install --accept-license] [--model ID] [--no-vision] [--deep] [--remote]");
                    std::process::exit(0);
                }
                s if s == "--remote"
                    || s.starts_with("--remote=")
                    || s.starts_with("--remote-title-tag=") => {}
                s => return Err(format!("unknown argument: {s}")),
            }
            i += 1;
        }
        if usize::from(result.script.is_some())
            + usize::from(result.once.is_some())
            + usize::from(result.install)
            > 1
        {
            return Err("choose one of --run, --once, --install".into());
        }
        Ok(result)
    }
    fn config(
        &self,
        base: &std::path::Path,
        control: &process::Control,
    ) -> process::Result<watch::Config> {
        let mut config = watch::Config::load(base, control)?;
        if let Some(model) = &self.model {
            config.model = model.clone();
        }
        config.no_vision = self.no_vision;
        if self.deep {
            config.deep_tests = true;
        }
        Ok(config)
    }
}
fn cli(options: &Options) -> process::Result<i32> {
    let base = std::env::current_dir().map_err(|e| e.to_string())?;
    let control = process::Control::default();
    let config = options.config(&base, &control)?;
    let notify: report::Notify = Arc::new(|update| match update {
        report::Update::Failed(s) => eprintln!("RED: {s}"),
        report::Update::Model(s) => println!("model: {s}"),
        _ => {}
    });
    if options.install {
        uihub::install(&config.model, options.accept_license, &control, &notify)?;
        return Ok(0);
    }
    if let Some(path) = &options.script {
        return pipeline::run_quick(&base, &config, path, control, notify);
    }
    let branch = options.once.as_deref().unwrap_or("work");
    let tip = watch::tip(&base, &config, branch, &control)?;
    pipeline::run_once(&base, &config, branch, &tip, control, notify).map(|b| report::exit_code(&b.verdict))
}
fn main() {
    let options = match Options::parse() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("ci: {e}");
            std::process::exit(1);
        }
    };
    if options.script.is_some() || options.once.is_some() || options.install {
        let code = match cli(&options) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("ci: {e}");
                1
            }
        };
        std::process::exit(code);
    }
    app::run();
}
