use std::fmt;
use std::sync::Mutex;
use wirefilter::{
    ExecutionContext, FunctionArgs, FunctionDefinition,
    FunctionDefinitionContext, FunctionParam, LhsValue, Map, ParserSettings, SchemeBuilder,
    TypedMap, Type, CompoundType,
};

struct LazyMapState {
    map: Mutex<Option<Map<'static>>>,
}

impl LazyMapState {
    fn new() -> Self {
        Self {
            map: Mutex::new(None),
        }
    }

    fn get_or_init<F>(&self, f: F) -> Map<'static>
    where
        F: FnOnce() -> Map<'static>,
    {
        let mut guard = self.map.lock().unwrap();
        if let Some(ref map) = *guard {
            map.clone()
        } else {
            let new_map = f();
            *guard = Some(new_map.clone());
            new_map
        }
    }
}

impl fmt::Debug for LazyMapState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LazyMapState")
            .field("initialized", &self.map.lock().unwrap().is_some())
            .finish()
    }
}

impl Clone for LazyMapState {
    fn clone(&self) -> Self {
        let map = self.map.lock().unwrap();
        Self {
            map: Mutex::new(map.clone()),
        }
    }
}

#[derive(Debug, Clone)]
struct LazyHeaderFunction;

impl FunctionDefinition for LazyHeaderFunction {
    fn context(&self) -> Option<FunctionDefinitionContext> {
        Some(FunctionDefinitionContext::new(LazyMapState::new()))
    }

    fn check_param(
        &self,
        _settings: &ParserSettings,
        params: &mut dyn ExactSizeIterator<Item = FunctionParam<'_>>,
        _next_param: &FunctionParam<'_>,
        _ctx: Option<&mut FunctionDefinitionContext>,
    ) -> Result<(), wirefilter::FunctionParamError> {
        let index = params.len();
        if index == 0 {
            Ok(())
        } else {
            unreachable!()
        }
    }

    fn return_type(
        &self,
        _params: &mut dyn ExactSizeIterator<Item = FunctionParam<'_>>,
        _ctx: Option<&FunctionDefinitionContext>,
    ) -> Type {
        Type::Map(CompoundType::from(Type::Bytes))
    }

    fn arg_count(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }

    fn compile(
        &self,
        _params: &mut dyn ExactSizeIterator<Item = FunctionParam<'_>>,
        ctx: Option<FunctionDefinitionContext>,
    ) -> Box<dyn for<'i, 'a> Fn(FunctionArgs<'i, 'a>) -> Option<LhsValue<'a>> + Sync + Send + 'static>
    {
        let state = ctx
            .expect("LazyHeaderFunction requires context")
            .downcast::<LazyMapState>()
            .expect("Invalid context type");

        Box::new(move |_args| {
            let map = state.get_or_init(|| {
                let mut typed_map: TypedMap<'static, &str> = TypedMap::new();
                typed_map.insert(b"Content-Type", "text/plain");
                typed_map.insert(b"Host", "example.com");
                typed_map.into()
            });

            Some(LhsValue::Map(map))
        })
    }
}

fn main() {
    let mut builder = SchemeBuilder::new();
    builder
        .add_function("request.header", LazyHeaderFunction)
        .unwrap();
    let scheme = builder.build();

    let filter = scheme
        .parse(r#"request.header()["Content-Type"] == "text/plain""#)
        .unwrap()
        .compile();

    let ctx = ExecutionContext::new(&scheme);

    println!("First execution: {:?}", filter.execute(&ctx).unwrap());
    println!("Second execution: {:?}", filter.execute(&ctx).unwrap());

    let filter2 = scheme
        .parse(r#"request.header()["Host"] == "example.com""#)
        .unwrap()
        .compile();

    println!("Filter2 execution: {:?}", filter2.execute(&ctx).unwrap());

    let filter3 = scheme
        .parse(r#"request.header()["Content-Type"] == "application/json""#)
        .unwrap()
        .compile();

    println!("Filter3 execution (should be false): {:?}", filter3.execute(&ctx).unwrap());
}
