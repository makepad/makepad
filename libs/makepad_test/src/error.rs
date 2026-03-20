use std::fmt;
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestError {
    message: String,
}

impl TestError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn from_panic_payload(payload: Box<dyn std::any::Any + Send>) -> Self {
        if let Some(message) = payload.downcast_ref::<String>() {
            return Self::new(format!("test panicked: {message}"));
        }
        if let Some(message) = payload.downcast_ref::<&'static str>() {
            return Self::new(format!("test panicked: {message}"));
        }
        Self::new("test panicked with a non-string payload")
    }
}

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TestError {}

impl From<String> for TestError {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for TestError {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<io::Error> for TestError {
    fn from(value: io::Error) -> Self {
        Self::new(value.to_string())
    }
}

pub type TestResult<T = ()> = Result<T, TestError>;

pub trait IntoTestResult {
    fn into_test_result(self) -> TestResult<()>;
}

impl IntoTestResult for () {
    fn into_test_result(self) -> TestResult<()> {
        Ok(())
    }
}

impl<E> IntoTestResult for Result<(), E>
where
    E: Into<TestError>,
{
    fn into_test_result(self) -> TestResult<()> {
        self.map_err(Into::into)
    }
}
