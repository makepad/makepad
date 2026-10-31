// A fn whose signature changed in one edit (a new fn) is edited again in a later one, together
// with a field it used being removed (Amp corpus, 2026-10-06: the later edit was refused with the
// first version's body).
use std::sync::Mutex;

struct App {
    name: String,
    opens: u32,
}

static APP: Mutex<Option<App>> = Mutex::new(None);

impl App {
    fn record(&mut self) -> String {
        self.opens += 1;
        format!("{} {}", self.name, self.opens)
    }
}

#[test]
fn run() {
    let mut g = APP.lock().unwrap();
    let app = g.get_or_insert_with(|| App { name: "amp".to_string(), opens: 0 });
    println!("out {}", app.record());
}
