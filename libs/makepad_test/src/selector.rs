use makepad_a11y::{query::{quote, Query}, Role};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    terms: Vec<String>,
    window: Option<String>,
}
impl Selector {
    pub fn all() -> Self { Self { terms: Vec::new(), window: Some("0".into()) } }
    /// A named widget's path suffix (semantic node ids use `raw("id=...")`).
    pub fn id(value: impl Into<String>) -> Self { Self::path(value) }
    pub fn path(value: impl Into<String>) -> Self { Self::all().term("path", value.into()) }
    pub fn role(value: Role) -> Self { Self::all().term("role", value.short().into()) }
    pub fn widget_type(value: impl Into<String>) -> Self { Self::all().term("class", value.into()) }
    pub fn raw(value: impl Into<String>) -> Self { Self { terms: vec![value.into()], ..Self::all() } }
    pub fn name(self, value: impl Into<String>) -> Self { self.term("name", value.into()) }
    pub fn text_exact(self, value: impl Into<String>) -> Self { self.name(value) }
    pub fn text_contains(self, value: impl Into<String>) -> Self { self.term("text~=", value.into()) }
    pub fn state(self, value: impl Into<String>) -> Self { self.term("state", value.into()) }
    pub fn nth(self, index: usize) -> Self { self.term("nth", index.to_string()) }
    pub fn window(mut self, value: impl Into<String>) -> Self { self.window = Some(value.into()); self }
    pub fn window_index(self, index: usize) -> Self { self.window(index.to_string()) }
    pub fn any_window(mut self) -> Self { self.window = None; self }
    fn term(mut self, key: &str, value: String) -> Self {
        self.terms.push(format!("{}{}{}", key, if key.ends_with('=') { "" } else { "=" }, quote(&value)));
        self
    }
    pub fn as_query(&self) -> String { self.terms.join(" ") }
    pub fn query(&self) -> Result<Query, String> { Query::parse(&self.as_query()) }
    pub(crate) fn describe(&self) -> String { self.as_query() }
    pub(crate) fn window_arg(&self) -> Option<&str> { self.window.as_deref() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_a11y::{Node, States};
    #[test]
    fn formats_queries() {
        assert_eq!(Selector::id("foo").as_query(), "path=\"foo\"");
        assert_eq!(Selector::widget_type("TextInput").as_query(), "class=\"TextInput\"");
        assert_eq!(Selector::raw("path=foo").as_query(), "path=foo");
    }
    #[test]
    fn matches_text_and_window_scopes() {
        let node = Node { class: "TextInput".into(), value: "hello world".into(), ..Default::default() };
        let selector = Selector::widget_type("TextInput").text_contains("hello").window_index(1);
        assert!(selector.query().unwrap().matches(&node));
        assert_eq!(selector.window_arg(), Some("1"));
        assert_eq!(selector.any_window().window_arg(), None);
    }
    #[test]
    fn raw_queries_search_semantic_fields() {
        let node = Node { role: Role::Button, name: "Save \"as\" \\ draft".into(), states: States::FOCUSED, ..Default::default() };
        assert!(Selector::role(Role::Button).name(&node.name).state("focused,enabled").query().unwrap().matches(&node));
        assert!(!Selector::raw("state=disabled").query().unwrap().matches(&node));
        assert!(Selector::raw("not_a_key=1").query().is_err());
    }
}
