use std::sync::Arc;
use wirefilter::{
    ExecutionContext, LhsValue, SchemeBuilder, SchemeStore, SimpleFunctionArgKind,
    SimpleFunctionDefinition, SimpleFunctionImpl, SimpleFunctionParam, Type,
};

fn double_int(args: wirefilter::FunctionArgs<'_, '_>) -> Option<LhsValue<'static>> {
    let val = args.next()?.ok()?;
    match val {
        LhsValue::Int(n) => Some(LhsValue::Int(n * 2)),
        _ => None,
    }
}

fn main() {
    let scheme_store: Arc<SchemeStore> = Arc::new(SchemeStore::new(false));

    let double_fn = SimpleFunctionDefinition {
        params: vec![SimpleFunctionParam {
            arg_kind: SimpleFunctionArgKind::Both,
            val_type: Type::Int,
        }],
        opt_params: vec![],
        return_type: Type::Int,
        implementation: SimpleFunctionImpl::new(double_int),
    };

    let mut builder1 = SchemeBuilder::new_with_scheme_store(scheme_store.clone());
    builder1.add_field("src.port", Type::Int).unwrap();
    builder1.add_function("double", double_fn.clone()).unwrap();
    let scheme1 = builder1.build();

    let mut builder2 = SchemeBuilder::new_with_scheme_store(scheme_store.clone());
    builder2.add_field("src.port", Type::Int).unwrap();
    builder2.add_field("dst.port", Type::Int).unwrap();
    builder2.add_function("double", double_fn.clone()).unwrap();
    let scheme2 = builder2.build();

    println!("Scheme1 function_count: {}", scheme1.function_count());
    println!("Scheme2 function_count: {}", scheme2.function_count());
    println!(
        "SchemeStore function_count: {}",
        scheme_store.function_count()
    );
    println!(
        "Shared scheme_store: {}",
        Arc::ptr_eq(scheme1.scheme_store(), scheme2.scheme_store())
    );

    let mut ctx = ExecutionContext::new(&scheme2);
    ctx.set_field_value(scheme2.get_field("src.port").unwrap(), LhsValue::Int(5))
        .unwrap();
    ctx.set_field_value(scheme2.get_field("dst.port").unwrap(), LhsValue::Int(80))
        .unwrap();

    let filter1 = scheme1
        .parse("double(src.port) > 8")
        .unwrap()
        .compile();
    let filter2 = scheme2
        .parse("double(src.port) > 8")
        .unwrap()
        .compile();
    let filter3 = scheme2
        .parse("double(dst.port) > 100")
        .unwrap()
        .compile();

    println!("filter1 (double(src.port) > 8) on ctx: {:?}", filter1.execute(&ctx).unwrap());
    println!("filter2 (double(src.port) > 8) on ctx: {:?}", filter2.execute(&ctx).unwrap());
    println!("filter3 (double(dst.port) > 100) on ctx: {:?}", filter3.execute(&ctx).unwrap());

    let ghost_fn = scheme1.parse("dst.port == 80");
    println!("Scheme1 parsing ghost field: {:?}", ghost_fn);
    assert!(ghost_fn.is_err());
}
