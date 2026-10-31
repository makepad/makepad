// Two versions of one package in one graph (old one renamed): each name reaches its own crate,
// `$crate` in each one's macro is that crate, and module_path!/CARGO_CRATE_NAME/type_name keep the
// package's crate name.
fn main() {
    println!("{} {} {} {}", mvold::VERSION, mvdep::VERSION, mvold::which!(), mvdep::which!());
    println!("{} {} {} {}", mvold::mpath(), mvdep::mpath(), mvold::cname(), mvdep::cname());
    println!("{} {} {}", mvdep::only_v2(), mv_mid::mid_value(), mv_mid::mvdep::VERSION);
    println!("{} {}", std::any::type_name::<mvold::Thing>(), std::any::type_name::<mvdep::Thing>());
}
