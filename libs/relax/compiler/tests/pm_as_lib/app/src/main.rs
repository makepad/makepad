fn main() {
    println!("pm_as_lib build: {} run: {} {}", env!("PM_BUILD"), pmlib::avail(), pmlib::tokens("x"));
}
