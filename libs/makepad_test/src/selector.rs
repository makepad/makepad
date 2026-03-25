use makepad_studio_protocol::WidgetSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
enum WindowTarget {
    Primary,
    Any,
    Id(String),
    Index(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    id: Option<String>,
    widget_type: Option<String>,
    raw: Option<String>,
    text_exact: Option<String>,
    text_contains: Option<String>,
    nth: Option<usize>,
    window: WindowTarget,
}

impl Selector {
    pub fn all() -> Self {
        Self {
            id: None,
            widget_type: None,
            raw: None,
            text_exact: None,
            text_contains: None,
            nth: None,
            window: WindowTarget::Primary,
        }
    }

    pub fn id(value: impl Into<String>) -> Self {
        let mut selector = Self::all();
        selector.id = Some(value.into());
        selector
    }

    pub fn widget_type(value: impl Into<String>) -> Self {
        let mut selector = Self::all();
        selector.widget_type = Some(value.into());
        selector
    }

    pub fn raw(value: impl Into<String>) -> Self {
        let mut selector = Self::all();
        selector.raw = Some(value.into());
        selector
    }

    pub fn text_exact(mut self, value: impl Into<String>) -> Self {
        self.text_exact = Some(value.into());
        self
    }

    pub fn text_contains(mut self, value: impl Into<String>) -> Self {
        self.text_contains = Some(value.into());
        self
    }

    pub fn nth(mut self, index: usize) -> Self {
        self.nth = Some(index);
        self
    }

    pub fn window(mut self, value: impl Into<String>) -> Self {
        self.window = WindowTarget::Id(value.into());
        self
    }

    pub fn window_index(mut self, index: usize) -> Self {
        self.window = WindowTarget::Index(index);
        self
    }

    pub fn any_window(mut self) -> Self {
        self.window = WindowTarget::Any;
        self
    }

    pub fn as_query(&self) -> String {
        if let Some(value) = &self.raw {
            return value.clone();
        }
        if let Some(value) = &self.id {
            return format!("id:{value}");
        }
        if let Some(value) = &self.widget_type {
            return format!("type:{value}");
        }
        self.describe()
    }

    pub(crate) fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(value) = &self.id {
            parts.push(format!("id:{value}"));
        }
        if let Some(value) = &self.widget_type {
            parts.push(format!("type:{value}"));
        }
        if let Some(value) = &self.text_exact {
            parts.push(format!("text_exact:{value}"));
        }
        if let Some(value) = &self.text_contains {
            parts.push(format!("text_contains:{value}"));
        }
        if let Some(value) = &self.raw {
            parts.push(format!("raw:{value}"));
        }
        match &self.window {
            WindowTarget::Primary => {}
            WindowTarget::Any => parts.push("window:any".to_string()),
            WindowTarget::Id(value) => parts.push(format!("window:{value}")),
            WindowTarget::Index(index) => parts.push(format!("window_index:{index}")),
        }
        if let Some(index) = self.nth {
            parts.push(format!("nth:{index}"));
        }
        if parts.is_empty() {
            "<all widgets>".to_string()
        } else {
            parts.join(", ")
        }
    }

    pub(crate) fn nth_index(&self) -> Option<usize> {
        self.nth
    }

    pub(crate) fn matches(
        &self,
        widget: &WidgetSnapshot,
        primary_window_id: &str,
        primary_window_index: usize,
    ) -> bool {
        if !self.window_matches(widget, primary_window_id, primary_window_index) {
            return false;
        }
        if let Some(value) = &self.id {
            if widget.id != *value {
                return false;
            }
        }
        if let Some(value) = &self.widget_type {
            if widget.widget_type != *value {
                return false;
            }
        }
        if let Some(value) = &self.text_exact {
            if !Self::text_fields(widget)
                .iter()
                .any(|field| *field == value)
            {
                return false;
            }
        }
        if let Some(value) = &self.text_contains {
            if !Self::text_fields(widget)
                .iter()
                .any(|field| field.contains(value))
            {
                return false;
            }
        }
        if let Some(value) = &self.raw {
            if !Self::matches_raw(widget, value) {
                return false;
            }
        }
        true
    }

    fn window_matches(
        &self,
        widget: &WidgetSnapshot,
        primary_window_id: &str,
        primary_window_index: usize,
    ) -> bool {
        match &self.window {
            WindowTarget::Primary => {
                widget.window_index == primary_window_index
                    || (!primary_window_id.is_empty() && widget.window_id == primary_window_id)
            }
            WindowTarget::Any => true,
            WindowTarget::Id(value) => widget.window_id == *value,
            WindowTarget::Index(index) => widget.window_index == *index,
        }
    }

    fn text_fields(widget: &WidgetSnapshot) -> Vec<&str> {
        let mut fields = Vec::new();
        if let Some(value) = widget.text.as_deref() {
            fields.push(value);
        }
        if let Some(value) = widget.value.as_deref() {
            fields.push(value);
        }
        if let Some(value) = widget.selected.as_deref() {
            fields.push(value);
        }
        fields
    }

    fn searchable_fields(widget: &WidgetSnapshot) -> Vec<&str> {
        let mut fields = vec![
            widget.id.as_str(),
            widget.widget_type.as_str(),
            widget.window_id.as_str(),
        ];
        fields.extend(Self::text_fields(widget));
        fields
    }

    fn matches_raw(widget: &WidgetSnapshot, raw: &str) -> bool {
        let raw = raw.trim();
        if let Some(value) = raw.strip_prefix("id:") {
            return widget.id == value.trim();
        }
        if let Some(value) = raw.strip_prefix("type:") {
            return widget.widget_type == value.trim();
        }
        if let Some(value) = raw.strip_prefix("text:") {
            let value = value.trim();
            return Self::text_fields(widget)
                .iter()
                .any(|field| *field == value);
        }
        if let Some(value) = raw.strip_prefix("value:") {
            return widget.value.as_deref() == Some(value.trim());
        }
        if let Some(value) = raw.strip_prefix("window:") {
            return widget.window_id == value.trim();
        }
        Self::searchable_fields(widget)
            .iter()
            .any(|field| field.contains(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::Selector;
    use makepad_studio_protocol::WidgetSnapshot;

    fn snapshot() -> WidgetSnapshot {
        WidgetSnapshot {
            id: "panel_input".to_string(),
            widget_type: "TextInput".to_string(),
            window_id: "panel_window".to_string(),
            window_index: 1,
            visible: true,
            enabled: true,
            x: 10,
            y: 20,
            width: 30,
            height: 40,
            text: None,
            value: Some("hello world".to_string()),
            checked: None,
            selected: None,
        }
    }

    #[test]
    fn formats_queries() {
        assert_eq!(Selector::id("foo").as_query(), "id:foo");
        assert_eq!(
            Selector::widget_type("TextInput").as_query(),
            "type:TextInput"
        );
        assert_eq!(Selector::raw("id:foo").as_query(), "id:foo");
    }

    #[test]
    fn matches_text_and_window_scopes() {
        let widget = snapshot();
        assert!(Selector::widget_type("TextInput")
            .text_contains("hello")
            .window("panel_window")
            .matches(&widget, "main_window", 0));
        assert!(!Selector::widget_type("TextInput")
            .window("main_window")
            .matches(&widget, "main_window", 0));
    }

    #[test]
    fn raw_queries_search_snapshot_fields() {
        let widget = snapshot();
        assert!(Selector::raw("hello world")
            .any_window()
            .matches(&widget, "main_window", 0));
        assert!(Selector::raw("value:hello world")
            .any_window()
            .matches(&widget, "main_window", 0));
        assert!(Selector::raw("window:panel_window").any_window().matches(
            &widget,
            "main_window",
            0
        ));
    }
}
