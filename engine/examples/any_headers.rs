use wirefilter::{AnyFunction, Array, ExecutionContext, LookupFunction, Scheme};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = Scheme! {
        http.request.headers.names: Array(Bytes),
        http.request.headers.values: Array(Bytes),
    };

    builder.add_function("any", AnyFunction::default())?;
    builder.add_function("lookup", LookupFunction::new())?;

    let scheme = builder.build();

    let mut ctx = ExecutionContext::new(&scheme);

    let arr: Array = Array::from_iter(["Accept", "Content-Type", "User-Agent"]);
    ctx.set_field_value(scheme.get_field("http.request.headers.names").unwrap(), arr)?;

    let arr2: Array = Array::from_iter(["application/json", "text/html", "Mozilla/5.0"]);
    ctx.set_field_value(
        scheme.get_field("http.request.headers.values").unwrap(),
        arr2,
    )?;

    let lookup_ast = scheme.parse(
        r#"any(lookup("Content-Type", http.request.headers.names, http.request.headers.values)[*] == "text/html")"#,
    )?;
    println!("Lookup AST: {:?}", lookup_ast);
    let lookup_filter = lookup_ast.compile();
    println!(
        "Lookup result (found 'text/html'): {:?}",
        lookup_filter.execute(&ctx)?
    );

    let multi_lookup_ast = scheme.parse(
        r#"any(lookup("Accept", "Content-Type", http.request.headers.names, http.request.headers.values)[*] == "application/json")"#,
    )?;
    println!("Multi-lookup AST: {:?}", multi_lookup_ast);
    let multi_lookup_filter = multi_lookup_ast.compile();
    println!(
        "Multi-lookup result (found 'application/json'): {:?}",
        multi_lookup_filter.execute(&ctx)?
    );

    Ok(())
}
