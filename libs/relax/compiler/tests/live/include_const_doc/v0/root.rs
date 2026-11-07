// Two consts that are include_str! of a file, in a module whose items have derives and a body
// with macro calls above them: a save of the included files alone re-evaluates the consts and
// their readers (a patch). Their type, `&str`, did not change: the expression a macro made lies
// past the file's text, so the type compared up to it took in the expansions before it, the
// first const's text among them (an app's two such consts took the restart rung for an edit of
// the files they include, 11-07).
mod lib_vm;

#[test]
fn run() {
    println!("out lib {}", lib_vm::lib_text());
    println!("out n {}", lib_vm::count());
    println!("out guide {}", lib_vm::guide());
}
