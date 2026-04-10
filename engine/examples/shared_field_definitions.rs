use std::sync::Arc;
use wirefilter::{ExecutionContext, FieldDefinitions, LhsValue, SchemeBuilder, Type};

fn main() {
    let field_definitions: Arc<FieldDefinitions> = Arc::new(FieldDefinitions::new(false));

    let mut builder1 = SchemeBuilder::new_with_field_definitions(field_definitions.clone());
    builder1.add_field("ip.src", Type::Ip).unwrap();
    builder1.add_field("ip.dst", Type::Ip).unwrap();
    builder1.add_field("port", Type::Int).unwrap();
    let scheme1 = builder1.build();

    let mut builder2 = SchemeBuilder::new_with_field_definitions(field_definitions.clone());
    builder2.add_field("ip.src", Type::Ip).unwrap();
    builder2.add_field("ip.dst", Type::Ip).unwrap();
    builder2.add_field("port", Type::Int).unwrap();
    builder2.add_field("ip.protocol", Type::Int).unwrap();
    let scheme2 = builder2.build();

    println!("Scheme1 owned field count: {}", scheme1.field_count());
    println!("Scheme2 owned field count: {}", scheme2.field_count());
    println!(
        "Total field count (shared): {}",
        field_definitions.field_count()
    );
    println!(
        "Shared field_definitions: {}",
        Arc::ptr_eq(scheme1.field_definitions(), scheme2.field_definitions())
    );

    assert!(scheme1.get_field("ip.src").is_ok());
    assert!(scheme1.get_field("ip.protocol").is_err());
    assert!(scheme2.get_field("ip.src").is_ok());
    assert!(scheme2.get_field("ip.protocol").is_ok());

    let mut ctx = ExecutionContext::new(&scheme2);

    ctx.set_field_value(
        scheme1.get_field("ip.src").unwrap(),
        LhsValue::Ip("1.2.3.4".parse().unwrap()),
    )
    .unwrap();

    ctx.set_field_value(scheme2.get_field("ip.protocol").unwrap(), LhsValue::Int(6))
        .unwrap();

    let filter1 = scheme1.parse("ip.src == 1.2.3.4").unwrap().compile();
    let filter2 = scheme2.parse("ip.src == 1.2.3.4").unwrap().compile();
    let filter3 = scheme2.parse("ip.protocol == 6").unwrap().compile();

    println!("Filter1 on ctx: {:?}", filter1.execute(&ctx).unwrap());
    println!("Filter2 on ctx: {:?}", filter2.execute(&ctx).unwrap());
    println!("Filter3 on ctx: {:?}", filter3.execute(&ctx).unwrap());

    let ghost_result = scheme1.parse("ip.protocol == 6");
    println!("Scheme1 parsing ghost field: {:?}", ghost_result);
    assert!(ghost_result.is_err());
}
