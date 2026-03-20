#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selector {
    Id(String),
    WidgetType(String),
    Raw(String),
}

impl Selector {
    pub fn id(value: impl Into<String>) -> Self {
        Self::Id(value.into())
    }

    pub fn widget_type(value: impl Into<String>) -> Self {
        Self::WidgetType(value.into())
    }

    pub fn raw(value: impl Into<String>) -> Self {
        Self::Raw(value.into())
    }

    pub fn as_query(&self) -> String {
        match self {
            Self::Id(value) => format!("id:{value}"),
            Self::WidgetType(value) => format!("type:{value}"),
            Self::Raw(value) => value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Selector;

    #[test]
    fn formats_queries() {
        assert_eq!(Selector::id("foo").as_query(), "id:foo");
        assert_eq!(
            Selector::widget_type("TextInput").as_query(),
            "type:TextInput"
        );
        assert_eq!(Selector::raw("id:foo").as_query(), "id:foo");
    }
}
