use std::sync::Arc;
use wirefilter::{ExecutionContext, FieldDefinitions, LhsValue, Scheme, SchemeBuilder, Type};

fn main() {
    let mut builder = SchemeBuilder::new();
    builder.add_field("ip.src", Type::Ip).unwrap();
    builder.add_field("ip.dst", Type::Ip).unwrap();
    builder.add_field("port", Type::Int).unwrap();
    let scheme1: Scheme = builder.build();

    let field_definitions: Arc<FieldDefinitions> = scheme1.field_definitions().clone();

    let builder2 = SchemeBuilder::with_field_definitions(field_definitions.clone());
    let scheme2: Scheme = builder2.build();

    println!("Scheme1 field count: {}", scheme1.field_count());
    println!("Scheme2 field count: {}", scheme2.field_count());
    println!("Shared field_definitions: {}", Arc::ptr_eq(scheme1.field_definitions(), scheme2.field_definitions()));

    let mut ctx1 = ExecutionContext::new(&scheme1);
    ctx1.set_field_value(
        scheme1.get_field("ip.src").unwrap(),
        LhsValue::Ip("1.2.3.4".parse().unwrap()),
    ).unwrap();

    let mut ctx2 = ExecutionContext::new(&scheme2);
    ctx2.set_field_value(
        scheme2.get_field("ip.src").unwrap(),
        LhsValue::Ip("5.6.7.8".parse().unwrap()),
    ).unwrap();

    let filter1 = scheme1.parse("ip.src == 1.2.3.4").unwrap().compile();
    let filter2 = scheme2.parse("ip.src == 5.6.7.8").unwrap().compile();

    println!("Filter1 on ctx1: {:?}", filter1.execute(&ctx1).unwrap());
    println!("Filter2 on ctx2: {:?}", filter2.execute(&ctx2).unwrap());
}
