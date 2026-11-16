mod kernel;

fn main() {
    let host = (kernel::work(50), kernel::work(400));
    println!("host {:016x} {:016x}", host.0, host.1);
    #[cfg(relax)]
    {
        let m = std::relax::Module::load_source("warm", "warm.rs", include_str!("kernel.rs"), &[]).expect("load warm module");
        let work: fn(u32) -> u64 = m.get("work").expect("get warm work");
        println!("module {:016x} {:016x}", work(50), work(400));
        m.unload();
    }
    #[cfg(not(relax))]
    println!("module {:016x} {:016x}", host.0, host.1);
}
