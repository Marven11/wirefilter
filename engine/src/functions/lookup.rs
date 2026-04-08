use crate::{
    Array, FunctionArgs, FunctionDefinition, FunctionDefinitionContext, FunctionParam,
    FunctionParamError, GetType, LhsValue, ParserSettings, Type,
};

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

/// A function which performs a lookup operation between arrays.
///
/// It takes at least 3 arguments:
/// - One or more search values
/// - A lookup array to search in
/// - A return array to return values from
///
/// For each occurrence of search values in the lookup array,
/// the corresponding value from the return array is returned.
#[derive(Debug, Default)]
pub struct LookupFunction;

impl LookupFunction {
    /// Creates a new LookupFunction instance.
    pub const fn new() -> Self {
        Self
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Array;

    #[test]
    fn test_lookup_fn_basic() {
        let lookup_arr = LhsValue::Array(Array::from_iter(["Accept", "Content-Type"]));
        let return_arr = LhsValue::Array(Array::from_iter(["json", "html"]));
        let mut args = vec![Ok(LhsValue::from("Content-Type")), Ok(lookup_arr), Ok(return_arr)].into_iter();
        let result = lookup_impl(&mut args);
        
        let expected = Array::from_iter(["html"]);
        assert_eq!(result, Some(LhsValue::Array(expected)));
    }

    #[test]
    fn test_lookup_fn_multi_search() {
        let lookup_arr = LhsValue::Array(Array::from_iter(["a", "b", "c"]));
        let return_arr = LhsValue::Array(Array::from_iter(["1", "2", "3"]));
        let mut args = vec![
            Ok(LhsValue::from("a")),
            Ok(LhsValue::from("c")),
            Ok(lookup_arr),
            Ok(return_arr),
        ].into_iter();
        let result = lookup_impl(&mut args);
        
        let expected = Array::from_iter(["1", "3"]);
        assert_eq!(result, Some(LhsValue::Array(expected)));
    }

    #[test]
    fn test_lookup_fn_not_found() {
        let lookup_arr = LhsValue::Array(Array::from_iter(["a", "b"]));
        let return_arr = LhsValue::Array(Array::from_iter(["1", "2"]));
        let mut args = vec![Ok(LhsValue::from("z")), Ok(lookup_arr), Ok(return_arr)].into_iter();
        let result = lookup_impl(&mut args);
        
        let expected = Array::new(Type::Bytes);
        assert_eq!(result, Some(LhsValue::Array(expected)));
    }
}
