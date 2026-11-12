// `Span::file` and `Span::local_file` (stable since 1.88), as a template derive calls
// `Span::call_site().local_file()` to find the directory of the file that invokes it.
use proc_macro::{Span, TokenStream};

/// A string literal: "local_file ok" when the call site's local file is absent or a file on disk
/// whose name `file()` ends with, as rustc gives them.
#[proc_macro]
pub fn call_site_file(_: TokenStream) -> TokenStream {
    let span = Span::call_site();
    let shown = span.file();
    let ok = match span.local_file() {
        None => true,
        Some(path) => path.is_file() && shown.ends_with(path.file_name().and_then(|n| n.to_str()).unwrap_or("?")),
    };
    format!("{:?}", if ok { "local_file ok" } else { "local_file WRONG" }).parse().unwrap()
}
