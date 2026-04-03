use super::{Error, Regex, RegexBuilder};

#[derive(Debug, Clone)]
pub(crate) struct StubRegex;

impl Regex for StubRegex {
    fn is_match(&self, _input: &[u8]) -> bool {
        unimplemented!("Engine was built without regex support")
    }

    fn clone_boxed(&self) -> Box<dyn Regex> {
        Box::new(self.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub struct RegexDefaultBuilder;

impl RegexBuilder for RegexDefaultBuilder {
    fn build(&self, _pattern: &str) -> Result<Box<dyn Regex>, Error> {
        Ok(Box::new(StubRegex))
    }

    fn clone_box(&self) -> Box<dyn RegexBuilder> {
        Box::new(self.clone())
    }
}
