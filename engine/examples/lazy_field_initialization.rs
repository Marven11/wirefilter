use std::sync::Arc;
use wirefilter::{ExecutionContext, LhsValue, SchemeBuilder, SchemeStore, Type};

fn init_network_fields(ctx: &mut ExecutionContext<'static, ()>, scheme: &wirefilter::Scheme) {
    let was_set = ctx.set_field_value_lazy(
        scheme.get_field("ip.src").unwrap(),
        || LhsValue::Ip("10.0.0.1".parse().unwrap()),
    ).unwrap();
    assert!(was_set);

    let was_set = ctx.set_field_value_lazy(
        scheme.get_field("ip.dst").unwrap(),
        || LhsValue::Ip("10.0.0.2".parse().unwrap()),
    ).unwrap();
    assert!(was_set);

    let was_set = ctx.set_field_value_lazy(
        scheme.get_field("port").unwrap(),
        || LhsValue::Int(443),
    ).unwrap();
    assert!(was_set);
}

fn init_protocol_fields(ctx: &mut ExecutionContext<'static, ()>, scheme: &wirefilter::Scheme) {
    let was_set = ctx.set_field_value_lazy(
        scheme.get_field("ip.src").unwrap(),
        || LhsValue::Ip("10.0.0.1".parse().unwrap()),
    ).unwrap();
    assert!(!was_set);

    let was_set = ctx.set_field_value_lazy(
        scheme.get_field("port").unwrap(),
        || LhsValue::Int(443),
    ).unwrap();
    assert!(!was_set);

    let was_set = ctx.set_field_value_lazy(
        scheme.get_field("ip.protocol").unwrap(),
        || LhsValue::Int(6),
    ).unwrap();
    assert!(was_set);
}

fn main() {
    let scheme_store: Arc<SchemeStore> = Arc::new(SchemeStore::new(false));

    let mut builder1 = SchemeBuilder::new_with_scheme_store(scheme_store.clone());
    builder1.add_field("ip.src", Type::Ip).unwrap();
    builder1.add_field("ip.dst", Type::Ip).unwrap();
    builder1.add_field("port", Type::Int).unwrap();
    let scheme1 = builder1.build();

    let mut builder2 = SchemeBuilder::new_with_scheme_store(scheme_store.clone());
    builder2.add_field("ip.src", Type::Ip).unwrap();
    builder2.add_field("ip.dst", Type::Ip).unwrap();
    builder2.add_field("port", Type::Int).unwrap();
    builder2.add_field("ip.protocol", Type::Int).unwrap();
    let scheme2 = builder2.build();

    let mut ctx = ExecutionContext::new(&scheme2);

    init_network_fields(&mut ctx, &scheme1);
    init_protocol_fields(&mut ctx, &scheme2);

    let filter1 = scheme1.parse("ip.src == 10.0.0.1").unwrap().compile();
    let filter2 = scheme2.parse("ip.src == 10.0.0.1").unwrap().compile();
    let filter3 = scheme2.parse("ip.protocol == 6").unwrap().compile();

    println!("Filter1 on ctx: {:?}", filter1.execute(&ctx).unwrap());
    println!("Filter2 on ctx: {:?}", filter2.execute(&ctx).unwrap());
    println!("Filter3 on ctx: {:?}", filter3.execute(&ctx).unwrap());
}
