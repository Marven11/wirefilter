use super::{Error, Regex, RegexBuilder};
use regex_automata::MatchKind;
use regex_automata::nfa::thompson::WhichCaptures;

#[derive(Clone)]
pub(crate) struct DefaultRegex(regex_automata::meta::Regex);

impl Regex for DefaultRegex {
    #[inline]
    fn is_match(&self, input: &[u8]) -> bool {
        self.0.is_match(input)
    }

    fn clone_boxed(&self) -> Box<dyn Regex> {
        Box::new(self.clone())
    }
}

/// Default regex builder using `regex_automata`.
#[derive(Clone, Debug)]
pub struct RegexDefaultBuilder {
    /// Approximate size limit of the compiled regular expression.
    pub compiled_size_limit: usize,
    /// Approximate size of the cache used by the DFA of a regex.
    pub dfa_size_limit: usize,
}

impl Default for RegexDefaultBuilder {
    #[inline]
    fn default() -> Self {
        Self {
            compiled_size_limit: 10 * (1 << 20),
            dfa_size_limit: 2 * (1 << 20),
        }
    }
}

impl RegexDefaultBuilder {
    #[inline]
    fn syntax_config() -> regex_automata::util::syntax::Config {
        regex_automata::util::syntax::Config::new()
            .unicode(false)
            .utf8(false)
    }

    #[inline]
    fn meta_config(&self) -> regex_automata::meta::Config {
        regex_automata::meta::Config::new()
            .match_kind(MatchKind::LeftmostFirst)
            .utf8_empty(false)
            .dfa(false)
            .nfa_size_limit(Some(self.compiled_size_limit))
            .onepass(false)
            .dfa_size_limit(Some(self.compiled_size_limit))
            .hybrid_cache_capacity(self.dfa_size_limit)
            .which_captures(WhichCaptures::Implicit)
    }
}

impl RegexBuilder for RegexDefaultBuilder {
    fn build(&self, pattern: &str) -> Result<Box<dyn Regex>, Error> {
        ::regex_automata::meta::Builder::new()
            .configure(self.meta_config())
            .syntax(Self::syntax_config())
            .build(pattern)
            .map(|re| Box::new(DefaultRegex(re)) as Box<dyn Regex>)
            .map_err(|err| {
                if let Some(limit) = err.size_limit() {
                    Error::CompiledTooBig(limit)
                } else if let Some(syntax) = err.syntax_error() {
                    Error::Syntax(syntax.to_string())
                } else {
                    Error::Other(err.to_string())
                }
            })
    }

    fn clone_box(&self) -> Box<dyn RegexBuilder> {
        Box::new(self.clone())
    }
}

#[test]
fn test_compiled_size_limit() {
    use super::{RegexExpr, RegexFormat};

    const COMPILED_SIZE_LIMIT: usize = 1024 * 1024;
    let builder = RegexDefaultBuilder {
        compiled_size_limit: COMPILED_SIZE_LIMIT,
        ..Default::default()
    };
    assert_eq!(
        RegexExpr::new(".{4079,65535}", RegexFormat::Literal, &builder),
        Err(Error::CompiledTooBig(COMPILED_SIZE_LIMIT))
    );
}
