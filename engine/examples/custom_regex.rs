use std::sync::Arc;
use wirefilter::{
    ExecutionContext, ParserSettings, Regex as WirefilterRegex, RegexError, RegexProvider,
    SchemeBuilder, Type,
};

#[derive(Debug)]
struct RegexCrateProvider;

struct WrappedRegex(regex::Regex);

impl WirefilterRegex for WrappedRegex {
    fn is_match(&self, input: &[u8]) -> bool {
        let s = std::str::from_utf8(input).unwrap_or("");
        self.0.is_match(s)
    }
}

impl RegexProvider for RegexCrateProvider {
    fn lookup_regex(&self, pattern: &str) -> Result<Arc<dyn WirefilterRegex>, RegexError> {
        regex::Regex::new(pattern)
            .map(|re| Arc::new(WrappedRegex(re)) as Arc<dyn WirefilterRegex>)
            .map_err(|e| RegexError::Syntax(e.to_string()))
    }
}

fn main() {
    let mut builder = SchemeBuilder::new();
    builder.add_field("http.host", Type::Bytes).unwrap();
    let scheme = builder.build();

    let settings = ParserSettings {
        regex_provider: Arc::new(RegexCrateProvider),
        ..Default::default()
    };
    let parser = wirefilter::FilterParser::with_settings(&scheme, settings);

    let ast = parser
        .parse("http.host matches \"^(www\\.)?example\\.com$\"")
        .unwrap();
    println!("Parsed: {ast:#?}");

    let filter = ast.compile();

    let mut ctx = ExecutionContext::new(&scheme);
    ctx.set_field_value(
        scheme.get_field("http.host").unwrap(),
        "www.example.com",
    )
    .unwrap();
    println!("matches www.example.com: {}", filter.execute(&ctx).unwrap());

    ctx.set_field_value(scheme.get_field("http.host").unwrap(), "other.com")
        .unwrap();
    println!("matches other.com: {}", filter.execute(&ctx).unwrap());
}
