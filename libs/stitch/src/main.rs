use {
    std::fs,
    stitch::{Engine, Linker, Module, Store, Val},
};

fn main() {
    let engine = Engine::new();
    let mut store = Store::new(engine);
    let bytes = fs::read("/Users/ejpbruel/Downloads/fib32.wasm").unwrap();
    let module = Module::new(store.engine(), &bytes).unwrap();
    let linker = Linker::new();
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let func = instance.exported_func("fib").unwrap();
    let mut results = [Val::I32(0)];
    func.call(&mut store, &[Val::I32(42)], &mut results)
        .unwrap();
}
