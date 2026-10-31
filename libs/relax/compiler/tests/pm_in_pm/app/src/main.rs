use pmpm_hello::Hello;

#[derive(pmpm_inner::Hello)]
struct App;

fn main() {
    println!("{} {}", pmpm_outer::outer!(), <App as Hello>::hello());
}
