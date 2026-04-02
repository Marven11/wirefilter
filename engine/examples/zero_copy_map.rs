use std::collections::HashMap;
use wirefilter::{ExecutionContext, Scheme, TypedMap};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scheme = Scheme! {
        headers: Map(Bytes),
    }
    .build();

    let headers_field = scheme.get_field("headers").unwrap();

    let filter = scheme
        .parse(r#"headers["host"] == "example.com" && headers["user-agent"] contains "curl""#)?
        .compile();

    let mut headers = HashMap::new();
    headers.insert("host", "example.com");
    headers.insert("user-agent", "curl/7.88");
    headers.insert("accept", "*/*");

    {
        let mut typed_map: TypedMap<'_, &str> = TypedMap::new();
        for (k, v) in &headers {
            typed_map.insert(k.as_bytes(), *v);
        }

        let mut ctx = ExecutionContext::new(&scheme);
        ctx.set_field_value(headers_field, typed_map)?;

        println!("Filter matches: {:?}", filter.execute(&ctx)?);
    }

    Ok(())
}
