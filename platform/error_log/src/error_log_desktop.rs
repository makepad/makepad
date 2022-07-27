#[macro_export]
macro_rules!log {
    ( $ ( $t: tt) *) => {
        println!("{}:{} - {}",file!(),line!(),format!($($t)*))
    }
}

#[macro_export]
macro_rules!error {
    ( $ ( $t: tt) *) => {
        eprintln!("{}:{} - {}",file!(),line!(),format!($($t)*))
    }
}

