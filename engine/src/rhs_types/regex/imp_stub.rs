use super::{Error, Regex, RegexProvider};
use crate::RegexFormat;
use std::sync::Arc;
use thiserror::Error;

/// Dummy regex error.
#[derive(Debug, PartialEq, Error)]
pub enum Error {}

pub(crate) struct StubRegex;

impl Regex for StubRegex {
    fn is_match(&self, _text: &[u8]) -> bool {
        unimplemented!("Engine was built without regex support")
    }
}

/// Default regex provider for stub mode.
#[derive(Debug, Default)]
pub struct RegexDefaultProvider;

impl RegexProvider for RegexDefaultProvider {
    fn lookup_regex(&self, _pattern: &str) -> Result<Arc<dyn Regex>, Error> {
        Ok(Arc::new(StubRegex))
    }
}
