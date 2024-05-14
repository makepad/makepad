use {
    makepad_stitch::{Engine, Linker, Module, Store, Val, ValType},
    std::{env, fs},
};

fn main() {
    let mut args = env::args();
    args.next().unwrap();
    let wasm_file = args.next().unwrap();
    let func_name = args.next().unwrap();
    let args: Vec<_> = args.collect();
    let engine = Engine::new();
    let mut store = Store::new(engine);
    let bytes = fs::read(wasm_file).unwrap();
    let module = Module::new(store.engine(), &bytes).unwrap();
    let linker = Linker::new();
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let func = instance.exported_func(&func_name).unwrap();
    let args: Vec<_> = func
        .type_(&store)
        .params()
        .iter()
        .zip(args)
        .map(|(type_, string)| parse_val(*type_, &string))
        .collect();
    let mut results: Vec<_> = func
        .type_(&store)
        .results()
        .iter()
        .map(|type_| Val::default(*type_))
        .collect();
    func.call(&mut store, &args, &mut results).unwrap();
    for result in results {
        print_val(result);
    }
}

fn parse_val(type_: ValType, string: &str) -> Val {
    match type_ {
        ValType::I32 => string.parse::<i32>().unwrap().into(),
        ValType::I64 => string.parse::<i64>().unwrap().into(),
        ValType::F32 => string.parse::<f32>().unwrap().into(),
        ValType::F64 => string.parse::<f64>().unwrap().into(),
        ValType::FuncRef => unimplemented!(),
        ValType::ExternRef => unimplemented!(),
    }
}

fn print_val(val: Val) {
    match val {
        Val::I32(val) => println!("{}", val),
        Val::I64(val) => println!("{}", val),
        Val::F32(val) => println!("{}", val),
        Val::F64(val) => println!("{}", val),
        Val::FuncRef(_) => unimplemented!(),
        Val::ExternRef(_) => unimplemented!(),
    }
}
