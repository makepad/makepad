//! `sqlq <db> [sql] [--explain] [--quote] [--param V]...`
//!
//! Runs read-only SQL against a SQLite database with this engine. Without SQL
//! it prints the header, WAL state and schema.

use makepad_sqlite::{exec, Database, Value};
use std::path::Path;

fn usage() -> ! {
    eprintln!(
        "usage: sqlq <database> [sql] [--explain] [--quote] [--param <value>]...\n\
         \n\
         Values for --param are text by default; use int:N, real:X, blob:HEX or null."
    );
    std::process::exit(2);
}

fn parse_param(s: &str) -> Value {
    if s == "null" {
        return Value::Null;
    }
    if let Some(rest) = s.strip_prefix("int:") {
        return rest
            .parse::<i64>()
            .map(Value::Integer)
            .unwrap_or(Value::Null);
    }
    if let Some(rest) = s.strip_prefix("real:") {
        return rest.parse::<f64>().map(Value::Real).unwrap_or(Value::Null);
    }
    if let Some(rest) = s.strip_prefix("blob:") {
        let mut bytes = Vec::with_capacity(rest.len() / 2);
        let b = rest.as_bytes();
        let mut i = 0;
        while i + 1 < b.len() {
            bytes.push(u8::from_str_radix(&rest[i..i + 2], 16).unwrap_or(0));
            i += 2;
        }
        return Value::Blob(bytes);
    }
    Value::text(s.strip_prefix("text:").unwrap_or(s))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let path = Path::new(&args[0]);
    let mut sql: Option<String> = None;
    let mut explain = false;
    let mut quote = false;
    let mut params: Vec<Value> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--explain" => explain = true,
            "--quote" => quote = true,
            "--param" => {
                i += 1;
                if i >= args.len() {
                    usage();
                }
                params.push(parse_param(&args[i]));
            }
            other if other.starts_with("--") => usage(),
            other => {
                if sql.is_some() {
                    usage();
                }
                sql = Some(other.to_string());
            }
        }
        i += 1;
    }

    let mut db = match Database::open(path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("sqlq: {e}");
            std::process::exit(1);
        }
    };

    let Some(sql) = sql else {
        print_schema(&mut db);
        return;
    };

    let stmt = match db.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sqlq: {e}");
            std::process::exit(1);
        }
    };
    if explain {
        print!("{}", stmt.explain());
        return;
    }
    let mut count = 0u64;
    let render = |v: &Value| -> String {
        if quote {
            exec::quote_value(v)
        } else {
            match v {
                Value::Null => String::new(),
                Value::Blob(_) => exec::quote_value(v),
                other => exec::to_text(other),
            }
        }
    };
    let result = stmt.for_each(&mut db, &params, |row| {
        let line: Vec<String> = row.iter().map(render).collect();
        println!("{}", line.join("|"));
        count += 1;
        Ok(true)
    });
    if let Err(e) = result {
        eprintln!("sqlq: {e}");
        std::process::exit(1);
    }
    let _ = count;
}

fn print_schema(db: &mut Database) {
    let header = db.pager().header().clone();
    let (frames, page_count) = (db.pager().wal_frames(), db.pager().page_count());
    println!(
        "page size {} usable {} pages {} encoding {:?} user_version {} wal frames {}",
        header.page_size,
        header.usable_size(),
        page_count,
        header.text_encoding,
        header.user_version,
        frames
    );
    for t in &db.schema().tables {
        match &t.unsupported {
            Some(why) => println!("table {} (root {}): {}", t.name, t.root_page, why),
            None => {
                let cols: Vec<String> = t
                    .columns
                    .iter()
                    .map(|c| format!("{} {}", c.name, c.decl_type))
                    .collect();
                println!("table {} (root {}): {}", t.name, t.root_page, cols.join(", "));
                for i in &t.indexes {
                    let cols: Vec<&str> = i
                        .columns
                        .iter()
                        .map(|ic| t.columns[ic.column].name.as_str())
                        .collect();
                    println!(
                        "  index {} (root {}){}: {}",
                        i.name,
                        i.root_page,
                        if i.unique { " unique" } else { "" },
                        cols.join(", ")
                    );
                }
            }
        }
    }
}
