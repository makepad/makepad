//~ E0412
//~ help: import it: `use crate::widgets::WidgetActionData;`
mod widgets { pub struct WidgetActionData; pub struct WidgetActionCast; }
fn f(_a: &WidgetActionData) {}
fn main() {}
