use makepad_ci_judge::Judge;
use makepad_strict_json::{obj, s, Value};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.len() % 2 != 0 {
        eprintln!("usage: makepad-ci-judge <image> <prompt-file> [<image> <prompt-file>]...");
        return ExitCode::from(64);
    }
    let judge = Judge::start();
    if let Some(why) = judge.unreachable() {
        println!("{}", obj(vec![("unreachable", s(why))]).to_json());
        return ExitCode::from(2);
    }
    for pair in args.chunks_exact(2) {
        let image = Path::new(&pair[0]);
        let prompt = Path::new(&pair[1]);
        let result = std::fs::read_to_string(prompt)
            .map_err(|e| format!("{}: {e}", prompt.display()))
            .and_then(|text| judge.judge(image, &text));
        let mut line = match result {
            Ok(verdict) => verdict.json(),
            Err(e) => obj(vec![("err", s(e))]),
        };
        if let Value::Obj(fields) = &mut line {
            fields.push(("image".into(), s(image.display().to_string())));
        }
        println!("{}", line.to_json());
    }
    ExitCode::SUCCESS
}
