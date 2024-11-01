// this stub is necessary because some platforms require building
// as dll (mobile / wasm) and some require to be built as executable
// unfortunately cargo doesn't facilitate this without a main.rs stub
fn main(){
    makepad_example_ai_docs::app::app_main()
}
