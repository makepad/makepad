use std::io::Read;

fn main() {
    let result = (|| {
        let mut input = String::new();
        std::io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_string(&mut input)
            .map_err(|error| error.to_string())?;
        if input.len() > 4 * 1024 * 1024 {
            return Err("Push request exceeds the policy limit".to_owned());
        }
        let repository = std::env::current_dir().map_err(|error| error.to_string())?;
        makepad_studio::iteration_git::validate_push_policy(&repository, &input)
    })();
    if let Err(error) = result {
        eprintln!("Studio blocked push: {error}");
        std::process::exit(1);
    }
}
