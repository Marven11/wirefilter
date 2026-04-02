use wirefilter::{
    Array, FunctionArgs, FunctionDefinition, FunctionDefinitionContext,
    FunctionParam, FunctionParamError, GetType, LhsValue, ParserSettings, Scheme, Type,
};



#[derive(Debug, Default)]
pub struct LookupFunction;

impl LookupFunction {
    pub const fn new() -> Self {
        Self
    }
}

fn lookup_impl<'a>(args: FunctionArgs<'_, 'a>) -> Option<LhsValue<'a>> {
    let args: Vec<_> = args.collect();
    let args_len = args.len();

    if args_len < 3 {
        panic!("lookup requires at least 3 arguments");
    }

    let (lookup_array_arg, return_array_arg) = (&args[args_len - 2], &args[args_len - 1]);

    let search_values: Vec<_> = args[..args_len - 2]
        .iter()
        .filter_map(|arg| arg.as_ref().ok())
        .collect();

    let lookup_array = match lookup_array_arg {
        Ok(LhsValue::Array(arr)) => arr,
        Err(Type::Array(_)) => return None,
        _ => unreachable!(),
    };

    let return_array = match return_array_arg {
        Ok(LhsValue::Array(arr)) => arr,
        Err(Type::Array(_)) => return None,
        _ => unreachable!(),
    };

    let return_type = return_array.value_type();
    let mut result = Array::new(return_type);

    for (lookup_idx, lookup_val) in lookup_array.iter().enumerate() {
        if search_values.iter().any(|search_val| lookup_val == *search_val) {
            if let Some(return_val) = return_array.get(lookup_idx) {
                let val_type = result.value_type();
                let mut vec = result.into_vec();
                vec.push(return_val.clone());
                result = Array::try_from_vec(val_type, vec).unwrap();
            }
        }
    }

    Some(LhsValue::Array(result))
}

impl FunctionDefinition for LookupFunction {
    fn check_param(
        &self,
        _: &ParserSettings,
        _: &mut dyn ExactSizeIterator<Item = FunctionParam<'_>>,
        _: &FunctionParam<'_>,
        _: Option<&mut FunctionDefinitionContext>,
    ) -> Result<(), FunctionParamError> {
        Ok(())
    }

    fn return_type(
        &self,
        params: &mut dyn ExactSizeIterator<Item = FunctionParam<'_>>,
        _: Option<&FunctionDefinitionContext>,
    ) -> Type {
        let mut last_type = Type::Array(Type::Bytes.into());
        while let Some(param) = params.next() {
            last_type = param.get_type();
        }
        last_type
    }

    fn arg_count(&self) -> (usize, Option<usize>) {
        (3, None)
    }

    fn compile<'s>(
        &'s self,
        _: &mut dyn ExactSizeIterator<Item = FunctionParam<'_>>,
        _: Option<FunctionDefinitionContext>,
    ) -> Box<dyn for<'i, 'a> Fn(FunctionArgs<'i, 'a>) -> Option<LhsValue<'a>> + Sync + Send + 'static>
    {
        Box::new(lookup_impl)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = Scheme! {
        http.request.headers.names: Array(Bytes),
        http.request.headers.values: Array(Bytes),
    };

    builder.add_function("any", wirefilter::AnyFunction::default())?;
    builder.add_function("lookup", LookupFunction::new())?;

    let scheme = builder.build();

    let mut ctx = wirefilter::ExecutionContext::new(&scheme);

    let arr: Array = Array::from_iter(["Accept", "Content-Type", "User-Agent"]);
    ctx.set_field_value(scheme.get_field("http.request.headers.names").unwrap(), arr)?;

    let arr2: Array = Array::from_iter(["application/json", "text/html", "Mozilla/5.0"]);
    ctx.set_field_value(scheme.get_field("http.request.headers.values").unwrap(), arr2)?;

    let lookup_ast = scheme.parse(
        r#"any(lookup("Content-Type", http.request.headers.names, http.request.headers.values)[*] == "text/html")"#,
    )?;
    println!("Lookup AST: {:?}", lookup_ast);
    let lookup_filter = lookup_ast.compile();
    println!("Lookup result (found 'text/html'): {:?}", lookup_filter.execute(&ctx)?);

    let multi_lookup_ast = scheme.parse(
        r#"any(lookup("Accept", "Content-Type", http.request.headers.names, http.request.headers.values)[*] == "application/json")"#,
    )?;
    println!("Multi-lookup AST: {:?}", multi_lookup_ast);
    let multi_lookup_filter = multi_lookup_ast.compile();
    println!("Multi-lookup result (found 'application/json'): {:?}", multi_lookup_filter.execute(&ctx)?);

    Ok(())
}
