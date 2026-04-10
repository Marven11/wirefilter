use crate::ast::parse::{FilterParser, ParseError, ParserSettings};
use crate::ast::{FilterAst, FilterValueAst};
use crate::functions::FunctionDefinition;
use crate::lex::{Lex, LexErrorKind, LexResult, LexWith, expect, span, take_while};
use crate::list_matcher::ListDefinition;
use crate::types::{GetType, RhsValue, Type};
use fnv::FnvBuildHasher;
use serde::de::Visitor;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::convert::TryFrom;
use std::fmt::{self, Debug, Formatter};
use std::hash::{Hash, Hasher};
use std::iter::Iterator;
use parking_lot::{RwLock, RwLockReadGuard, MappedRwLockReadGuard};
use std::sync::Arc;
use thiserror::Error;

/// An error that occurs if two underlying [schemes](struct@Scheme)
/// don't match.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("underlying schemes do not match")]
pub struct SchemeMismatchError;

/// Enum representing either:
/// * An array index with [`FieldIndex::ArrayIndex`]
/// * A map key with [`FieldIndex::MapKey`]
///
/// ```
/// #[allow(dead_code)]
/// enum FieldIndex {
///     ArrayIndex(u32),
///     MapKey(String),
/// }
/// ```
#[derive(Debug, PartialEq, Eq, Clone, Hash, Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum FieldIndex {
    /// Index into an Array
    ArrayIndex(u32),

    /// Key into a Map
    MapKey(String),

    /// Map each element by applying a function or a comparison
    MapEach,
}

impl<'i> Lex<'i> for FieldIndex {
    fn lex(input: &'i str) -> LexResult<'i, Self> {
        if let Ok(input) = expect(input, "*") {
            return Ok((FieldIndex::MapEach, input));
        }

        // The token inside an [] can be either an integer index into an Array
        // or a string key into a Map. The token is a key into a Map if it
        // starts and ends with "\"", otherwise an integer index or an error.
        let (rhs, rest) = match expect(input, "\"") {
            Ok(_) => RhsValue::lex_with(input, Type::Bytes),
            Err(_) => RhsValue::lex_with(input, Type::Int).map_err(|_| {
                (
                    LexErrorKind::ExpectedLiteral(
                        "expected quoted utf8 string or positive integer",
                    ),
                    input,
                )
            }),
        }?;

        match rhs {
            RhsValue::Int(i) => match u32::try_from(i) {
                Ok(u) => Ok((FieldIndex::ArrayIndex(u), rest)),
                Err(_) => Err((
                    LexErrorKind::ExpectedLiteral("expected positive integer as index"),
                    input,
                )),
            },
            RhsValue::Bytes(b) => match String::from_utf8(b.into()) {
                Ok(s) => Ok((FieldIndex::MapKey(s), rest)),
                Err(_) => Err((LexErrorKind::ExpectedLiteral("expected utf8 string"), input)),
            },
            _ => unreachable!(),
        }
    }
}

/// An error when an index is invalid for a type.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("cannot access index {index:?} for type {actual:?}")]
pub struct IndexAccessError {
    /// Index that could not be accessed.
    pub index: FieldIndex,
    /// Provided value type.
    pub actual: Type,
}

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
/// A structure to represent a field inside a [`Scheme`](struct@Scheme).
pub struct FieldRef<'s> {
    scheme_store: &'s Arc<SchemeStore>,
    index: usize,
}

impl Serialize for FieldRef<'_> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(ser)
    }
}

impl Debug for FieldRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl<'i, 's> LexWith<'i, &'s Scheme> for FieldRef<'s> {
    fn lex_with(input: &'i str, scheme: &'s Scheme) -> LexResult<'i, Self> {
        match Identifier::lex_with(input, scheme) {
            Ok((Identifier::Field(f), rest)) => Ok((f, rest)),
            Ok((Identifier::Function(_), rest)) => Err((
                LexErrorKind::UnknownField(UnknownFieldError),
                span(input, rest),
            )),
            Err((LexErrorKind::UnknownIdentifier, s)) => {
                Err((LexErrorKind::UnknownField(UnknownFieldError), s))
            }
            Err(err) => Err(err),
        }
    }
}

impl<'s> FieldRef<'s> {
    #[inline]
    fn field_def(&self) -> MappedRwLockReadGuard<'s, FieldDefinition> {
        self.scheme_store.index(self.index)
    }

    /// Returns the field's name as recorded in the [`Scheme`](struct@Scheme).
    #[inline]
    pub fn name(&self) -> MappedRwLockReadGuard<'s, str> {
        RwLockReadGuard::try_map(self.scheme_store.inner.read(), |inner: &SchemeStoreInner| inner.fields.get(self.index))
            .map(|guard| MappedRwLockReadGuard::map(guard, |fd| &fd.name[..]))
            .unwrap_or_else(|_| panic!("index {} out of bounds", self.index))
    }

    /// Get the field's index in the [`Scheme`](struct@Scheme) identifier's list.
    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns whether the field value is optional.
    #[inline]
    pub fn optional(&self) -> bool {
        self.field_def().optional
    }

    /// Returns the [`SchemeStore`](struct@SchemeStore) to which this field belongs to.
    #[inline]
    pub fn scheme_store(&self) -> &'s Arc<SchemeStore> {
        self.scheme_store
    }

    /// Converts to an owned [`Field`].
    #[inline]
    pub fn to_owned(&self) -> Field {
        Field {
            scheme_store: self.scheme_store.clone(),
            index: self.index,
        }
    }

    /// Reborrows the field relatively to the specified [`SchemeStore`] reference.
    ///
    /// Useful when you have a [`FieldRef`] borrowed from an owned [`Field`]
    /// but you need to extend/change it's lifetime.
    ///
    /// Panics if the field doesn't belong to the specified field definitions.
    #[inline]
    pub fn reborrow(self, scheme_store: &Arc<SchemeStore>) -> FieldRef<'_> {
        assert!(Arc::ptr_eq(self.scheme_store, scheme_store));

        FieldRef {
            scheme_store,
            index: self.index,
        }
    }
}

impl GetType for FieldRef<'_> {
    #[inline]
    fn get_type(&self) -> Type {
        self.field_def().ty
    }
}

impl PartialEq<Field> for FieldRef<'_> {
    #[inline]
    fn eq(&self, other: &Field) -> bool {
        self.eq(&other.as_ref())
    }
}

#[derive(PartialEq, Eq, Clone, Hash)]
/// A structure to represent a field inside a [`Scheme`](struct@Scheme).
pub struct Field {
    scheme_store: Arc<SchemeStore>,
    index: usize,
}

impl Serialize for Field {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(ser)
    }
}

impl Debug for Field {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Field {
    #[inline]
    fn field_def(&self) -> MappedRwLockReadGuard<'_, FieldDefinition> {
        self.scheme_store.index(self.index)
    }

    /// Returns the field's name as recorded in the [`Scheme`](struct@Scheme).
    #[inline]
    pub fn name(&self) -> MappedRwLockReadGuard<'_, str> {
        RwLockReadGuard::try_map(self.scheme_store.inner.read(), |inner: &SchemeStoreInner| inner.fields.get(self.index))
            .map(|guard| MappedRwLockReadGuard::map(guard, |fd| &fd.name[..]))
            .unwrap_or_else(|_| panic!("index {} out of bounds", self.index))
    }

    /// Get the field's index in the [`Scheme`](struct@Scheme) identifier's list.
    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns whether the field value is optional.
    #[inline]
    pub fn optional(&self) -> bool {
        self.field_def().optional
    }

    /// Returns the [`SchemeStore`](struct@SchemeStore) to which this field belongs to.
    #[inline]
    pub fn scheme_store(&self) -> &Arc<SchemeStore> {
        &self.scheme_store
    }

    /// Converts to a borrowed [`FieldRef`].
    #[inline]
    pub fn as_ref(&self) -> FieldRef<'_> {
        FieldRef {
            scheme_store: &self.scheme_store,
            index: self.index,
        }
    }
}

impl GetType for Field {
    #[inline]
    fn get_type(&self) -> Type {
        self.field_def().ty
    }
}

impl PartialEq<FieldRef<'_>> for Field {
    #[inline]
    fn eq(&self, other: &FieldRef<'_>) -> bool {
        self.as_ref().eq(other)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
/// A structure to represent a function inside a [`Scheme`](struct@Scheme).
pub struct FunctionRef<'s> {
    scheme_store: &'s Arc<SchemeStore>,
    index: usize,
}

impl Serialize for FunctionRef<'_> {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(ser)
    }
}

impl Debug for FunctionRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl<'i, 's> LexWith<'i, &'s Scheme> for FunctionRef<'s> {
    fn lex_with(input: &'i str, scheme: &'s Scheme) -> LexResult<'i, Self> {
        match Identifier::lex_with(input, scheme) {
            Ok((Identifier::Function(f), rest)) => Ok((f, rest)),
            Ok((Identifier::Field(_), rest)) => Err((
                LexErrorKind::UnknownFunction(UnknownFunctionError),
                span(input, rest),
            )),
            Err((LexErrorKind::UnknownIdentifier, s)) => {
                Err((LexErrorKind::UnknownFunction(UnknownFunctionError), s))
            }
            Err(err) => Err(err),
        }
    }
}

impl<'s> FunctionRef<'s> {
    /// Returns the function's name as recorded in the [`Scheme`](struct@Scheme).
    #[inline]
    pub fn name(&self) -> MappedRwLockReadGuard<'s, str> {
        MappedRwLockReadGuard::map(
            self.scheme_store.function_at(self.index),
            |f| &f.0[..],
        )
    }

    /// Get the function's index in the [`Scheme`](struct@Scheme) identifier's list.
    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the [`SchemeStore`](struct@SchemeStore) to which this function belongs to.
    #[inline]
    pub fn scheme_store(&self) -> &'s Arc<SchemeStore> {
        self.scheme_store
    }

    #[inline]
    pub(crate) fn as_definition(&self) -> MappedRwLockReadGuard<'s, dyn FunctionDefinition> {
        MappedRwLockReadGuard::map(
            self.scheme_store.function_at(self.index),
            |f| f.1.as_ref(),
        )
    }

    /// Converts to an owned [`Function`].
    #[inline]
    pub fn to_owned(&self) -> Function {
        Function {
            scheme_store: self.scheme_store.clone(),
            index: self.index,
        }
    }

    /// Reborrows the function relatively to the specified [`SchemeStore`] reference.
    ///
    /// Useful when you have a [`FunctionRef`] borrowed from an owned [`Function`]
    /// but you need to extend/change it's lifetime.
    ///
    /// Panics if the function doesn't belong to the specified scheme store.
    #[inline]
    pub fn reborrow(self, scheme_store: &Arc<SchemeStore>) -> FunctionRef<'_> {
        assert!(Arc::ptr_eq(self.scheme_store, scheme_store));

        FunctionRef {
            scheme_store,
            index: self.index,
        }
    }
}

impl PartialEq<Function> for FunctionRef<'_> {
    #[inline]
    fn eq(&self, other: &Function) -> bool {
        self.eq(&other.as_ref())
    }
}

#[derive(PartialEq, Eq, Clone, Hash)]
/// A structure to represent a function inside a [`Scheme`](struct@Scheme).
pub struct Function {
    scheme_store: Arc<SchemeStore>,
    index: usize,
}

impl Serialize for Function {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(ser)
    }
}

impl Debug for Function {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Function {
    /// Returns the function's name as recorded in the [`Scheme`](struct@Scheme).
    #[inline]
    pub fn name(&self) -> MappedRwLockReadGuard<'_, str> {
        MappedRwLockReadGuard::map(
            self.scheme_store.function_at(self.index),
            |f| &f.0[..],
        )
    }

    /// Get the function's index in the [`Scheme`](struct@Scheme) identifier's list.
    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the [`SchemeStore`](struct@SchemeStore) to which this function belongs to.
    #[inline]
    pub fn scheme_store(&self) -> &Arc<SchemeStore> {
        &self.scheme_store
    }

    #[inline]
    pub(crate) fn as_definition(&self) -> MappedRwLockReadGuard<'_, dyn FunctionDefinition> {
        MappedRwLockReadGuard::map(
            self.scheme_store.function_at(self.index),
            |f| f.1.as_ref(),
        )
    }

    /// Converts to a borrowed [`FunctionRef`].
    #[inline]
    pub fn as_ref(&self) -> FunctionRef<'_> {
        FunctionRef {
            scheme_store: &self.scheme_store,
            index: self.index,
        }
    }
}

impl PartialEq<FunctionRef<'_>> for Function {
    #[inline]
    fn eq(&self, other: &FunctionRef<'_>) -> bool {
        self.as_ref().eq(other)
    }
}

/// An enum to represent an entry inside a [`Scheme`](struct@Scheme).
/// It can be either a [`Field`](struct@Field) or a [`Function`](struct@Function).
#[derive(Debug)]
pub(crate) enum Identifier<'s> {
    /// Identifier is a [`Field`](struct@Field)
    Field(FieldRef<'s>),
    /// Identifier is a [`Function`](struct@Function)
    Function(FunctionRef<'s>),
}

impl<'i, 's> LexWith<'i, &'s Scheme> for Identifier<'s> {
    fn lex_with(mut input: &'i str, scheme: &'s Scheme) -> LexResult<'i, Self> {
        let initial_input = input;

        loop {
            input = take_while(input, "identifier character", |c| {
                c.is_ascii_alphanumeric() || c == '_'
            })?
            .1;

            match expect(input, ".") {
                Ok(rest) => input = rest,
                Err(_) => break,
            };
        }

        let name = span(initial_input, input);

        let field = scheme
            .get(name)
            .ok_or((LexErrorKind::UnknownIdentifier, name))?;

        Ok((field, input))
    }
}

/// An error that occurs if an unregistered field name was queried from a
/// [`Scheme`](struct@Scheme).
#[derive(Debug, PartialEq, Eq, Error)]
#[error("unknown field")]
pub struct UnknownFieldError;

/// An error that occurs if an unregistered function name was queried from a
/// [`Scheme`](struct@Scheme).
#[derive(Debug, PartialEq, Eq, Error)]
#[error("unknown function")]
pub struct UnknownFunctionError;

/// An error that occurs when previously defined field gets redefined.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("attempt to redefine field {0}")]
pub struct FieldRedefinitionError(String);

/// An error that occurs when a field is defined with a different type than an existing field with the same name.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("field '{name}' type mismatch: expected {expected:?}, got {got:?}")]
pub struct FieldTypeMismatchError {
    name: String,
    expected: Type,
    got: Type,
}

/// An error that occurs when previously defined function gets redefined.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("attempt to redefine function {0}")]
pub struct FunctionRedefinitionError(String);

/// An error that occurs when trying to redefine a field or function.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum IdentifierRedefinitionError {
    /// An error that occurs when previously defined field gets redefined.
    #[error("{0}")]
    Field(#[source] FieldRedefinitionError),

    /// An error that occurs when a field is defined with a different type.
    #[error("{0}")]
    FieldTypeMismatch(#[source] FieldTypeMismatchError),

    /// An error that occurs when previously defined function gets redefined.
    #[error("{0}")]
    Function(#[source] FunctionRedefinitionError),
}

#[derive(Clone, Copy, Debug)]
enum SchemeItem {
    Field(usize),
    Function(usize),
}

/// A structure to represent a list inside a [`scheme`](struct.Scheme.html).
///
/// See [`Scheme::get_list`](struct.Scheme.html#method.get_list).
#[derive(PartialEq, Eq, Clone, Copy, Hash)]
pub struct ListRef<'s> {
    scheme_store: &'s Arc<SchemeStore>,
    index: usize,
}

impl<'s> ListRef<'s> {
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    pub(crate) fn scheme_store(&self) -> &'s Arc<SchemeStore> {
        self.scheme_store
    }

    pub(crate) fn definition(&self) -> MappedRwLockReadGuard<'s, dyn ListDefinition> {
        MappedRwLockReadGuard::map(
            self.scheme_store.list_at(self.index),
            |f| f.1.as_ref(),
        )
    }

    /// Converts to an owned [`List`].
    #[inline]
    pub fn to_owned(&self) -> List {
        List {
            scheme_store: self.scheme_store.clone(),
            index: self.index,
        }
    }

    /// Reborrows the list relatively to the specified [`SchemeStore`] reference.
    #[inline]
    pub fn reborrow(self, scheme_store: &Arc<SchemeStore>) -> ListRef<'_> {
        assert!(Arc::ptr_eq(self.scheme_store, scheme_store));

        ListRef {
            scheme_store,
            index: self.index,
        }
    }
}

impl Debug for ListRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let list = self.scheme_store.list_at(self.index);
        write!(f, "{:?}", &*list)
    }
}

impl GetType for ListRef<'_> {
    #[inline]
    fn get_type(&self) -> Type {
        let list = self.scheme_store.list_at(self.index);
        list.0
    }
}

impl PartialEq<List> for ListRef<'_> {
    #[inline]
    fn eq(&self, other: &List) -> bool {
        self.eq(&other.as_ref())
    }
}

/// A structure to represent a list inside a [`scheme`](struct.Scheme.html).
///
/// See [`Scheme::get_list`](struct.Scheme.html#method.get_list).
#[derive(PartialEq, Eq, Clone, Hash)]
pub struct List {
    scheme_store: Arc<SchemeStore>,
    index: usize,
}

impl List {
    #[inline]
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    #[inline]
    pub(crate) fn scheme_store(&self) -> &Arc<SchemeStore> {
        &self.scheme_store
    }

    /// Converts to a borrowed [`ListRef`].
    #[inline]
    pub fn as_ref(&self) -> ListRef<'_> {
        ListRef {
            scheme_store: &self.scheme_store,
            index: self.index,
        }
    }
}

impl Debug for List {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let list = self.scheme_store.list_at(self.index);
        write!(f, "{:?}", &*list)
    }
}

impl GetType for List {
    #[inline]
    fn get_type(&self) -> Type {
        let list = self.scheme_store.list_at(self.index);
        list.0
    }
}

impl PartialEq<ListRef<'_>> for List {
    #[inline]
    fn eq(&self, other: &ListRef<'_>) -> bool {
        self.as_ref().eq(other)
    }
}

/// An error that occurs when previously defined list gets redefined.
#[derive(Debug, PartialEq, Eq, Error)]
#[error("attempt to redefine list for type {0:?}")]
pub struct ListRedefinitionError(Type);

type IdentifierName = Arc<str>;

#[derive(Debug, PartialEq, Clone, Hash)]
pub struct FieldDefinition {
    name: IdentifierName,
    ty: Type,
    optional: bool,
    scheme_id: u64,
}

impl FieldDefinition {
    /// Returns the name of the field.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the type of the field.
    #[inline]
    pub fn ty(&self) -> Type {
        self.ty
    }

    /// Returns whether the field is optional.
    #[inline]
    pub fn optional(&self) -> bool {
        self.optional
    }
}

#[derive(Debug)]
pub(crate) struct SchemeStoreInner {
    pub(crate) fields: Vec<FieldDefinition>,
    pub(crate) field_names: HashMap<Arc<str>, usize, FnvBuildHasher>,
    pub(crate) functions: Vec<(IdentifierName, Box<dyn FunctionDefinition>)>,
    pub(crate) function_names: HashMap<Arc<str>, usize, FnvBuildHasher>,
    pub(crate) lists: Vec<(Type, Box<dyn ListDefinition>)>,
    pub(crate) list_types: HashMap<Type, usize, FnvBuildHasher>,
    pub(crate) next_scheme_id: u64,
}

/// A shared collection of field definitions that can be referenced by multiple schemes.
#[derive(Debug)]
pub struct SchemeStore {
    inner: RwLock<SchemeStoreInner>,
    nil_not_equal_is_false: bool,
}

impl PartialEq for SchemeStore {
    fn eq(&self, other: &Self) -> bool {
        let a = self.inner.read();
        let b = other.inner.read();
        a.fields == b.fields && self.nil_not_equal_is_false == other.nil_not_equal_is_false
    }
}

impl Eq for SchemeStore {}

impl Hash for SchemeStore {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.read().fields.hash(state);
        self.nil_not_equal_is_false.hash(state);
    }
}

impl SchemeStore {
    /// Creates a new empty `SchemeStore`.
    ///
    /// When `nil_not_equal_is_false` is `true`, comparisons with `nil` values
    /// always return `false` instead of `nil`.
    pub fn new(nil_not_equal_is_false: bool) -> Self {
        SchemeStore {
            inner: RwLock::new(SchemeStoreInner {
                fields: Vec::new(),
                field_names: HashMap::default(),
                functions: Vec::new(),
                function_names: HashMap::default(),
                lists: Vec::new(),
                list_types: HashMap::default(),
                next_scheme_id: 0,
            }),
            nil_not_equal_is_false,
        }
    }

    /// Returns the number of fields.
    #[inline]
    pub fn field_count(&self) -> usize {
        self.inner.read().fields.len()
    }

    /// Returns a reference to the field definition at the given index.
    #[inline]
    pub fn get(&self, index: usize) -> Option<MappedRwLockReadGuard<'_, FieldDefinition>> {
        RwLockReadGuard::try_map(self.inner.read(), |inner| inner.fields.get(index)).ok()
    }

    /// Returns the field definition at the given index (panics if out of bounds).
    #[inline]
    pub fn index(&self, index: usize) -> MappedRwLockReadGuard<'_, FieldDefinition> {
        RwLockReadGuard::try_map(self.inner.read(), |inner| inner.fields.get(index))
            .unwrap_or_else(|_| panic!("index {index} out of bounds"))
    }

    /// Returns the field index for the given name.
    #[inline]
    pub fn get_field_index(&self, name: &str) -> Option<usize> {
        self.inner.read().field_names.get(name).copied()
    }

    /// Returns the nil_not_equal_behavior setting.
    #[inline]
    pub(crate) fn nil_not_equal_behavior(&self) -> bool {
        !self.nil_not_equal_is_false
    }

    /// Adds a field definition internally via write lock.
    pub(crate) fn get_or_add_field(&self, name: Arc<str>, ty: Type, optional: bool, scheme_id: u64) -> Result<usize, IdentifierRedefinitionError> {
        {
            let inner = self.inner.read();
            if let Some(&index) = inner.field_names.get(&name) {
                let existing_ty = inner.fields[index].ty;
                if existing_ty != ty {
                    return Err(IdentifierRedefinitionError::FieldTypeMismatch(FieldTypeMismatchError {
                        name: name.to_string(),
                        expected: existing_ty,
                        got: ty,
                    }));
                }
                return Ok(index);
            }
        }
        self.add_field_internal(name, ty, optional, scheme_id)
    }

    pub(crate) fn add_field_internal(&self, name: Arc<str>, ty: Type, optional: bool, scheme_id: u64) -> Result<usize, IdentifierRedefinitionError> {
        let mut inner = self.inner.write();
        match inner.field_names.get(&name) {
            Some(_) => Err(IdentifierRedefinitionError::Field(FieldRedefinitionError(name.to_string()))),
            None => {
                let index = inner.fields.len();
                inner.fields.push(FieldDefinition {
                    name: name.clone(),
                    ty,
                    optional,
                    scheme_id,
                });
                inner.field_names.insert(name, index);
                Ok(index)
            }
        }
    }

    pub(crate) fn allocate_scheme_id(&self) -> u64 {
        let mut inner = self.inner.write();
        let id = inner.next_scheme_id;
        inner.next_scheme_id += 1;
        id
    }

    /// Returns the number of functions registered in this [`SchemeStore`](struct@SchemeStore).
    pub fn function_count(&self) -> usize {
        self.inner.read().functions.len()
    }

    /// Returns the number of lists registered in this [`SchemeStore`](struct@SchemeStore).
    pub fn list_count(&self) -> usize {
        self.inner.read().lists.len()
    }

    pub(crate) fn get_or_add_function(&self, name: Arc<str>, function: Box<dyn FunctionDefinition>) -> Result<usize, IdentifierRedefinitionError> {
        {
            let inner = self.inner.read();
            if let Some(&index) = inner.function_names.get(&name) {
                return Ok(index);
            }
        }
        let mut inner = self.inner.write();
        match inner.function_names.get(&name) {
            Some(_) => Err(IdentifierRedefinitionError::Function(FunctionRedefinitionError(name.to_string()))),
            None => {
                let index = inner.functions.len();
                inner.functions.push((name.clone(), function));
                inner.function_names.insert(name, index);
                Ok(index)
            }
        }
    }

    pub(crate) fn get_or_add_list(&self, ty: Type, definition: Box<dyn ListDefinition>) -> Result<usize, ListRedefinitionError> {
        {
            let inner = self.inner.read();
            if let Some(&index) = inner.list_types.get(&ty) {
                return Ok(index);
            }
        }
        let mut inner = self.inner.write();
        match inner.list_types.get(&ty) {
            Some(_) => Err(ListRedefinitionError(ty)),
            None => {
                let index = inner.lists.len();
                inner.lists.push((ty, definition));
                inner.list_types.insert(ty, index);
                Ok(index)
            }
        }
    }

    pub(crate) fn function_at(&self, index: usize) -> MappedRwLockReadGuard<'_, (IdentifierName, Box<dyn FunctionDefinition>)> {
        RwLockReadGuard::try_map(self.inner.read(), |inner| inner.functions.get(index))
            .unwrap_or_else(|_| panic!("function index {index} out of bounds"))
    }

    pub(crate) fn list_at(&self, index: usize) -> MappedRwLockReadGuard<'_, (Type, Box<dyn ListDefinition>)> {
        RwLockReadGuard::try_map(self.inner.read(), |inner| inner.lists.get(index))
            .unwrap_or_else(|_| panic!("list index {index} out of bounds"))
    }

    pub(crate) fn list_at_index(&self, ty: &Type) -> Option<usize> {
        self.inner.read().list_types.get(ty).copied()
    }
}

#[derive(Debug)]
struct SchemeInner {
    scheme_store: Arc<SchemeStore>,
    items: HashMap<IdentifierName, SchemeItem, FnvBuildHasher>,
}

/// A builder for a [`Scheme`].
#[derive(Default, Debug)]
pub struct SchemeBuilder {
    shared_scheme_store: Option<Arc<SchemeStore>>,
    scheme_id: Option<u64>,
    fields: Vec<FieldDefinition>,
    functions: Vec<(IdentifierName, Box<dyn FunctionDefinition>)>,
    items: HashMap<IdentifierName, SchemeItem, FnvBuildHasher>,

    list_types: HashMap<Type, usize, FnvBuildHasher>,
    lists: Vec<(Type, Box<dyn ListDefinition>)>,

    nil_not_equal_is_false: bool,
}

impl SchemeBuilder {
    /// Creates a new scheme.
    pub fn new() -> Self {
        Default::default()
    }

    /// Creates a new scheme builder that shares the given [`SchemeStore`].
    ///
    /// The builder will use the same field definitions, allowing multiple
    /// schemes to share field data without duplication.
    pub fn new_with_scheme_store(scheme_store: Arc<SchemeStore>) -> Self {
        let scheme_id = scheme_store.allocate_scheme_id();

        SchemeBuilder {
            shared_scheme_store: Some(scheme_store.clone()),
            scheme_id: Some(scheme_id),
            fields: Vec::new(),
            functions: Vec::new(),
            items: HashMap::default(),
            list_types: HashMap::default(),
            lists: Vec::new(),
            nil_not_equal_is_false: scheme_store.nil_not_equal_is_false,
        }
    }

    fn add_field_full(
        &mut self,
        name: Arc<str>,
        ty: Type,
        optional: bool,
    ) -> Result<(), IdentifierRedefinitionError> {
        match self.items.entry(name.clone()) {
            Entry::Occupied(entry) => match entry.get() {
                SchemeItem::Field(_) => Err(IdentifierRedefinitionError::Field(
                    FieldRedefinitionError(entry.key().to_string()),
                )),
                SchemeItem::Function(_) => Err(IdentifierRedefinitionError::Function(
                    FunctionRedefinitionError(entry.key().to_string()),
                )),
            },
            Entry::Vacant(entry) => {
                let index = if let Some(ref fd) = self.shared_scheme_store {
                    let scheme_id = self.scheme_id.unwrap();
                    fd.get_or_add_field(name, ty, optional, scheme_id)?
                } else {
                    let index = self.fields.len();
                    self.fields.push(FieldDefinition {
                        name: entry.key().clone(),
                        ty,
                        optional,
                        scheme_id: 0,
                    });
                    index
                };
                entry.insert(SchemeItem::Field(index));
                Ok(())
            }
        }
    }

    /// Registers a field and its corresponding type.
    pub fn add_field<N: AsRef<str>>(
        &mut self,
        name: N,
        ty: Type,
    ) -> Result<(), IdentifierRedefinitionError> {
        self.add_field_full(name.as_ref().into(), ty, false)
    }

    /// Registers an optional field and its corresponding type.
    pub fn add_optional_field<N: AsRef<str>>(
        &mut self,
        name: N,
        ty: Type,
    ) -> Result<(), IdentifierRedefinitionError> {
        self.add_field_full(name.as_ref().into(), ty, true)
    }

    /// Registers a function
    pub fn add_function<N: AsRef<str>>(
        &mut self,
        name: N,
        function: impl FunctionDefinition + 'static,
    ) -> Result<(), IdentifierRedefinitionError> {
        match self.items.entry(name.as_ref().into()) {
            Entry::Occupied(entry) => match entry.get() {
                SchemeItem::Field(_) => Err(IdentifierRedefinitionError::Field(
                    FieldRedefinitionError(entry.key().to_string()),
                )),
                SchemeItem::Function(_) => Err(IdentifierRedefinitionError::Function(
                    FunctionRedefinitionError(entry.key().to_string()),
                )),
            },
            Entry::Vacant(entry) => {
                let index = if let Some(ref ss) = self.shared_scheme_store {
                    ss.get_or_add_function(entry.key().clone(), Box::new(function))?
                } else {
                    let index = self.functions.len();
                    self.functions.push((entry.key().clone(), Box::new(function)));
                    index
                };
                entry.insert(SchemeItem::Function(index));
                Ok(())
            }
        }
    }

    /// Registers a new [`list`](trait.ListDefinition.html) for a given [`type`](enum.Type.html).
    pub fn add_list(
        &mut self,
        ty: Type,
        definition: impl ListDefinition + 'static,
    ) -> Result<(), ListRedefinitionError> {
        match self.list_types.entry(ty) {
            Entry::Occupied(entry) => Err(ListRedefinitionError(*entry.key())),
            Entry::Vacant(entry) => {
                let index = if let Some(ref ss) = self.shared_scheme_store {
                    ss.get_or_add_list(ty, Box::new(definition))?
                } else {
                    let index = self.lists.len();
                    self.lists.push((ty, Box::new(definition)));
                    index
                };
                entry.insert(index);
                Ok(())
            }
        }
    }

    /// Configures the behavior of not equal comparison against a nil value.
    ///
    /// Default behavior is to return `true` for `nil != <value>`.
    /// By calling this method with `false`, this behavior can be
    /// changed so that `nil != <value>` returns `false` instead.
    pub fn set_nil_not_equal_behavior(&mut self, behavior: bool) {
        self.nil_not_equal_is_false = !behavior;
    }

    /// Build a new [`Scheme`] from this builder.
    pub fn build(self) -> Scheme {
        let scheme_store = if let Some(fd) = self.shared_scheme_store {
            fd
        } else {
            let field_names: HashMap<Arc<str>, usize, FnvBuildHasher> = self.items
                .iter()
                .filter_map(|(name, item)| match item {
                    SchemeItem::Field(index) => Some((name.clone(), *index)),
                    SchemeItem::Function(_) => None,
                })
                .collect();
            let function_names: HashMap<Arc<str>, usize, FnvBuildHasher> = self.items
                .iter()
                .filter_map(|(name, item)| match item {
                    SchemeItem::Function(index) => Some((name.clone(), *index)),
                    SchemeItem::Field(_) => None,
                })
                .collect();
            Arc::new(SchemeStore {
                inner: RwLock::new(SchemeStoreInner {
                    fields: self.fields,
                    field_names,
                    functions: self.functions,
                    function_names,
                    lists: self.lists,
                    list_types: self.list_types,
                    next_scheme_id: 1,
                }),
                nil_not_equal_is_false: self.nil_not_equal_is_false,
            })
        };

        Scheme {
            inner: Arc::new(SchemeInner {
                scheme_store,
                items: self.items,
            }),
        }
    }
}

impl<N: AsRef<str>> FromIterator<(N, Type)> for SchemeBuilder {
    fn from_iter<T: IntoIterator<Item = (N, Type)>>(iter: T) -> Self {
        let mut builder = SchemeBuilder::new();
        for (name, ty) in iter {
            builder
                .add_field(name.as_ref(), ty)
                .map_err(|err| err.to_string())
                .unwrap();
        }
        builder
    }
}

/// The main registry for fields and their associated types.
///
/// This is necessary to provide typechecking for runtime values provided
/// to the [`crate::ExecutionContext`] and also to aid parser
/// in ambiguous contexts.
#[derive(Clone, Debug)]
pub struct Scheme {
    inner: Arc<SchemeInner>,
}

impl PartialEq for Scheme {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for Scheme {}

impl Hash for Scheme {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.inner).hash(state);
    }
}

#[derive(Deserialize, Serialize)]
struct SerdeField {
    #[serde(rename = "type")]
    ty: Type,
    optional: bool,
}

impl Serialize for Scheme {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let fields = self.fields();
        let mut map = serializer.serialize_map(Some(self.field_count()))?;
        for f in fields {
            map.serialize_entry(
                &*f.name(),
                &SerdeField {
                    ty: f.get_type(),
                    optional: f.optional(),
                },
            )?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Scheme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        struct FieldMapVisitor;

        impl<'de> Visitor<'de> for FieldMapVisitor {
            type Value = SchemeBuilder;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a wirefilter scheme")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut builder = SchemeBuilder::new();
                while let Some((name, SerdeField { ty, optional })) =
                    map.next_entry::<&str, SerdeField>()?
                {
                    builder
                        .add_field_full(name.into(), ty, optional)
                        .map_err(A::Error::custom)?;
                }

                Ok(builder)
            }
        }

        deserializer
            .deserialize_map(FieldMapVisitor)
            .map(|builder| builder.build())
    }
}

impl<'s> Scheme {
    /// Returns the [`identifier`](enum@Identifier) with the specified `name`.
    pub(crate) fn get(&'s self, name: &str) -> Option<Identifier<'s>> {
        self.inner.items.get(name).and_then(|item| match *item {
            SchemeItem::Field(index) => {
                Some(Identifier::Field(FieldRef {
                    scheme_store: &self.inner.scheme_store,
                    index,
                }))
            }
            SchemeItem::Function(index) => Some(Identifier::Function(FunctionRef {
                scheme_store: &self.inner.scheme_store,
                index,
            })),
        })
    }

    /// Returns the [`field`](struct@Field) with the specified `name`.
    pub fn get_field(&'s self, name: &str) -> Result<FieldRef<'s>, UnknownFieldError> {
        match self.get(name) {
            Some(Identifier::Field(f)) => Ok(f),
            _ => Err(UnknownFieldError),
        }
    }

    /// Iterates over fields registered in the [`scheme`](struct@Scheme).
    #[inline]
    pub fn fields(&'s self) -> impl Iterator<Item = FieldRef<'s>> + 's {
        let fd = &self.inner.scheme_store;
        let indices: Vec<usize> = self.inner.items.iter()
            .filter_map(|(_, item)| match item {
                SchemeItem::Field(index) => Some(*index),
                SchemeItem::Function(_) => None,
            })
            .collect();
        indices.into_iter().map(move |index| FieldRef {
            scheme_store: fd,
            index,
        })
    }

    /// Returns the number of fields owned by this [`scheme`](struct@Scheme).
    #[inline]
    pub fn field_count(&self) -> usize {
        self.inner.items.iter().filter(|(_, item)| matches!(item, SchemeItem::Field(_))).count()
    }


    /// Returns the [`SchemeStore`](struct@SchemeStore) shared by this scheme.
    #[inline]
    pub fn scheme_store(&self) -> &Arc<SchemeStore> {
        &self.inner.scheme_store
    }

    /// Returns the number of functions in the [`scheme`](struct@Scheme).
    #[inline]
    pub fn function_count(&self) -> usize {
        self.inner.items.iter().filter(|(_, item)| matches!(item, SchemeItem::Function(_))).count()
    }

    /// Returns the [`function`](struct@Function) with the specified `name`.
    pub fn get_function(&'s self, name: &str) -> Result<FunctionRef<'s>, UnknownFunctionError> {
        match self.get(name) {
            Some(Identifier::Function(f)) => Ok(f),
            _ => Err(UnknownFunctionError),
        }
    }

    /// Iterates over functions registered in the [`scheme`](struct@Scheme).
    #[inline]
    pub fn functions(&'s self) -> impl Iterator<Item = FunctionRef<'s>> + 's {
        let ss = &self.inner.scheme_store;
        let indices: Vec<usize> = self.inner.items.iter()
            .filter_map(|(_, item)| match item {
                SchemeItem::Function(index) => Some(*index),
                SchemeItem::Field(_) => None,
            })
            .collect();
        indices.into_iter().map(move |index| FunctionRef {
            scheme_store: ss,
            index,
        })
    }

    /// Creates a new parser with default settings.
    pub fn parser(&self) -> FilterParser<'_> {
        FilterParser::new(self)
    }

    /// Creates a new parser with the specified settings.
    pub fn parser_with_settings(&self, settings: ParserSettings) -> FilterParser<'_> {
        FilterParser::with_settings(self, settings)
    }

    /// Parses a filter expression into an AST form.
    pub fn parse<'i>(&'s self, input: &'i str) -> Result<FilterAst, ParseError<'i>> {
        FilterParser::new(self).parse(input)
    }

    /// Parses a value expression into an AST form.
    pub fn parse_value<'i>(&'s self, input: &'i str) -> Result<FilterValueAst, ParseError<'i>> {
        FilterParser::new(self).parse_value(input)
    }

    /// Returns the number of lists in the [`scheme`](struct@Scheme)
    #[inline]
    pub fn list_count(&self) -> usize {
        self.inner.scheme_store.list_count()
    }

    /// Returns the [`list`](struct.List.html) for a given [`type`](enum.Type.html).
    pub fn get_list(&self, ty: &Type) -> Option<ListRef<'_>> {
        self.inner.scheme_store.list_at_index(ty).map(move |index| ListRef {
            scheme_store: &self.inner.scheme_store,
            index,
        })
    }

    /// Iterates over all registered [`lists`](trait.ListDefinition.html).
    pub fn lists(&self) -> impl ExactSizeIterator<Item = ListRef<'_>> + use<'_> {
        (0..self.inner.scheme_store.list_count()).map(|index| ListRef {
            scheme_store: &self.inner.scheme_store,
            index,
        })
    }

    /// Returns the nil_not_equal_behavior setting.
    #[inline]
    pub fn nil_not_equal_behavior(&self) -> bool {
        self.inner.scheme_store.nil_not_equal_behavior()
    }
}

/// A convenience macro for constructing a [`SchemeBuilder`] with static contents.
#[macro_export]
macro_rules! Scheme {
    ($($ns:ident $(. $field:ident)*: $ty:ident $(($subty:tt $($rest:tt)*))?),* $(,)*) => {
        $crate::SchemeBuilder::from_iter([$(
            (
                concat!(stringify!($ns) $(, ".", stringify!($field))*),
                Scheme!($ty $(($subty $($rest)*))?),
            )
        ),*])
    };
    ($ty:ident $(($subty:tt $($rest:tt)*))?) => {$crate::Type::$ty$(((Scheme!($subty $($rest)*)).into()))?};
}

#[test]
fn test_parse_error() {
    use crate::ConcatFunction;
    use crate::types::{ExpectedTypeList, TypeMismatchError};
    use indoc::indoc;

    let mut builder = Scheme! {
        num: Int,
        str: Bytes,
        arr: Array(Bool),
    };

    builder
        .add_function("concat", ConcatFunction::new())
        .unwrap();

    let scheme = builder.build();

    {
        let err = scheme.parse("xyz").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::UnknownIdentifier,
                input: "xyz",
                line_number: 0,
                span_start: 0,
                span_len: 3
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:1):
                xyz
                ^^^ unknown identifier
                "#
            )
        );
    }

    {
        let err = scheme.parse("xyz\n").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::UnknownIdentifier,
                input: "xyz",
                line_number: 0,
                span_start: 0,
                span_len: 3
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:1):
                xyz
                ^^^ unknown identifier
                "#
            )
        );
    }

    {
        let err = scheme.parse("\n\n    xyz").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::UnknownIdentifier,
                input: "    xyz",
                line_number: 2,
                span_start: 4,
                span_len: 3
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (3:5):
                    xyz
                    ^^^ unknown identifier
                "#
            )
        );
    }

    {
        let err = scheme
            .parse(indoc!(
                r#"
                num == 10 or
                num == true or
                num == 20
                "#
            ))
            .unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ExpectedName("digit"),
                input: "num == true or",
                line_number: 1,
                span_start: 7,
                span_len: 7
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (2:8):
                num == true or
                       ^^^^^^^ expected digit
                "#
            )
        );
    }

    {
        let err = scheme
            .parse(indoc!(
                r#"
                arr and arr
                "#
            ))
            .unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::TypeMismatch(TypeMismatchError {
                    expected: Type::Bool.into(),
                    actual: Type::Array(Type::Bool.into()),
                }),
                input: "arr and arr",
                line_number: 0,
                span_start: 11,
                span_len: 0,
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:12):
                arr and arr
                           ^ expected value of type Bool, but got Array<Bool>
                "#
            )
        );
    }

    {
        let err = scheme.parse_value(indoc!(r" arr[*] ")).unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::TypeMismatch(TypeMismatchError {
                    expected: Type::Bool.into(),
                    actual: Type::Array(Type::Bool.into()),
                }),
                input: " arr[*] ",
                line_number: 0,
                span_start: 1,
                span_len: 6,
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:2):
                 arr[*] 
                 ^^^^^^ expected value of type Bool, but got Array<Bool>
                "#
            )
        );
    }

    {
        let err = scheme.parse(indoc!(r"str in {")).unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::EOF,
                input: "str in {",
                line_number: 0,
                span_start: 8,
                span_len: 0,
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:9):
                str in {
                        ^ unrecognised input
                "#
            )
        );
    }

    {
        let err = scheme.parse(indoc!(r#"str in {"a""#)).unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::EOF,
                input: r#"str in {"a""#,
                line_number: 0,
                span_start: 11,
                span_len: 0,
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:12):
                str in {"a"
                           ^ unrecognised input
                "#
            )
        );
    }

    {
        let err = scheme.parse(indoc!(r"num in {")).unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ExpectedName("digit"),
                input: "num in {",
                line_number: 0,
                span_start: 8,
                span_len: 0,
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:9):
                num in {
                        ^ expected digit
                "#
            )
        );
    }

    {
        let err = scheme.parse(indoc!(r"concat(0, 0) == 0")).unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::InvalidArgumentType {
                    index: 0,
                    mismatch: TypeMismatchError {
                        expected: ExpectedTypeList::from(
                            crate::functions::concat::EXPECTED_TYPES.into_iter()
                        ),
                        actual: Type::Int,
                    },
                },
                input: "concat(0, 0) == 0",
                line_number: 0,
                span_start: 7,
                span_len: 1,
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:8):
                concat(0, 0) == 0
                       ^ invalid type of argument #0: expected value of type Bytes or Array<_>, but got Int
                "#
            )
        );
    }
}

#[test]
fn test_parse_error_in_op() {
    use cidr::errors::NetworkParseError;
    use indoc::indoc;
    use std::net::IpAddr;
    use std::str::FromStr;

    let scheme = &Scheme! {
        num: Int,
        bool: Bool,
        str: Bytes,
        ip: Ip,
        str_arr: Array(Bytes),
        str_map: Map(Bytes),
    }
    .build();

    {
        let err = scheme.parse("bool in {0}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::EOF,
                input: "bool in {0}",
                line_number: 0,
                span_start: 4,
                span_len: 7
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:5):
                bool in {0}
                    ^^^^^^^ unrecognised input
                "#
            )
        );
    }

    {
        let err = scheme.parse("bool in {127.0.0.1}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::EOF,
                input: "bool in {127.0.0.1}",
                line_number: 0,
                span_start: 4,
                span_len: 15
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:5):
                bool in {127.0.0.1}
                    ^^^^^^^^^^^^^^^ unrecognised input
                "#
            )
        );
    }

    {
        let err = scheme.parse("bool in {\"test\"}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::EOF,
                input: "bool in {\"test\"}",
                line_number: 0,
                span_start: 4,
                span_len: 12
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:5):
                bool in {"test"}
                    ^^^^^^^^^^^^ unrecognised input
                "#
            )
        );
    }

    {
        let err = scheme.parse("num in {127.0.0.1}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ExpectedName("digit"),
                input: "num in {127.0.0.1}",
                line_number: 0,
                span_start: 11,
                span_len: 7
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:12):
                num in {127.0.0.1}
                           ^^^^^^^ expected digit
                "#
            )
        );
    }

    {
        let err = scheme.parse("num in {\"test\"}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ExpectedName("digit"),
                input: "num in {\"test\"}",
                line_number: 0,
                span_start: 8,
                span_len: 7
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:9):
                num in {"test"}
                        ^^^^^^^ expected digit
                "#
            )
        );
    }
    {
        let err = scheme.parse("ip in {666}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ParseNetwork(
                    IpAddr::from_str("666")
                        .map_err(NetworkParseError::AddrParseError)
                        .unwrap_err()
                ),
                input: "ip in {666}",
                line_number: 0,
                span_start: 7,
                span_len: 3
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:8):
                ip in {666}
                       ^^^ couldn't parse address in network: invalid IP address syntax
                "#
            )
        );
    }
    {
        let err = scheme.parse("ip in {\"test\"}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ExpectedName("IP address character"),
                input: "ip in {\"test\"}",
                line_number: 0,
                span_start: 7,
                span_len: 7
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:8):
                ip in {"test"}
                       ^^^^^^^ expected IP address character
                "#
            )
        );
    }

    {
        let err = scheme.parse("str in {0}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ParseInt {
                    err: u8::from_str_radix("0}", 16).unwrap_err(),
                    radix: 16,
                },
                input: "str in {0}",
                line_number: 0,
                span_start: 8,
                span_len: 2
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:9):
                str in {0}
                        ^^ invalid digit found in string while parsing with radix 16
                "#
            )
        );
    }

    {
        let err = scheme.parse("str in {127.0.0.1}").unwrap_err();
        assert_eq!(
            err,
            ParseError {
                kind: LexErrorKind::ExpectedName("byte separator"),
                input: "str in {127.0.0.1}",
                line_number: 0,
                span_start: 10,
                span_len: 1
            }
        );
        assert_eq!(
            err.to_string(),
            indoc!(
                r#"
                Filter parsing error (1:11):
                str in {127.0.0.1}
                          ^ expected byte separator
                "#
            )
        );
    }

    for pattern in &["0", "127.0.0.1", "\"test\""] {
        {
            let filter = format!("str_arr in {{{pattern}}}");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Array(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }

        {
            let filter = format!("str_map in {{{pattern}}}");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Map(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }
    }
}

#[test]
fn test_parse_error_ordering_op() {
    let scheme = &Scheme! {
        num: Int,
        bool: Bool,
        str: Bytes,
        ip: Ip,
        str_arr: Array(Bytes),
        str_map: Map(Bytes),
    }
    .build();

    for op in &["eq", "ne", "ge", "le", "gt", "lt"] {
        {
            let filter = format!("num {op} 127.0.0.1");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::EOF,
                    input: &filter,
                    line_number: 0,
                    span_start: 10,
                    span_len: 6
                }
            );
        }

        {
            let filter = format!("num {op} \"test\"");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::ExpectedName("digit"),
                    input: &filter,
                    line_number: 0,
                    span_start: 7,
                    span_len: 6
                }
            );
        }
        {
            let filter = format!("str {op} 0");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::CountMismatch {
                        name: "character",
                        actual: 1,
                        expected: 2,
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 7,
                    span_len: 1
                }
            );
        }

        {
            let filter = format!("str {op} 256");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::ExpectedName("byte separator"),
                    input: &filter,
                    line_number: 0,
                    span_start: 9,
                    span_len: 1
                }
            );
        }

        {
            let filter = format!("str {op} 127.0.0.1");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::ExpectedName("byte separator"),
                    input: &filter,
                    line_number: 0,
                    span_start: 9,
                    span_len: 1,
                }
            );
        }

        {
            let filter = format!("str_arr {op} 0");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Array(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }

        {
            let filter = format!("str_arr {op} \"test\"");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Array(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }

        {
            let filter = format!("str_arr {op} 127.0.0.1");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Array(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }

        {
            let filter = format!("str_map {op} 0");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Map(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }

        {
            let filter = format!("str_map {op} \"test\"");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Map(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }

        {
            let filter = format!("str_map {op} 127.0.0.1");
            let err = scheme.parse(&filter).unwrap_err();
            assert_eq!(
                err,
                ParseError {
                    kind: LexErrorKind::UnsupportedOp {
                        lhs_type: Type::Map(Type::Bytes.into())
                    },
                    input: &filter,
                    line_number: 0,
                    span_start: 8,
                    span_len: 2
                }
            );
        }
    }
}

#[test]
fn test_field() {
    let scheme = &Scheme! {
        x: Bytes,
        x.y.z0: Int,
        is_TCP: Bool,
        map: Map(Bytes)
    }
    .build();

    assert_ok!(
        FieldRef::lex_with("x;", scheme),
        scheme.get_field("x").unwrap(),
        ";"
    );

    assert_ok!(
        FieldRef::lex_with("x.y.z0-", scheme),
        scheme.get_field("x.y.z0").unwrap(),
        "-"
    );

    assert_ok!(
        FieldRef::lex_with("is_TCP", scheme),
        scheme.get_field("is_TCP").unwrap(),
        ""
    );

    assert_err!(
        FieldRef::lex_with("x..y", scheme),
        LexErrorKind::ExpectedName("identifier character"),
        ".y"
    );

    assert_err!(
        FieldRef::lex_with("x.#", scheme),
        LexErrorKind::ExpectedName("identifier character"),
        "#"
    );

    assert_err!(
        FieldRef::lex_with("x.y.z;", scheme),
        LexErrorKind::UnknownField(UnknownFieldError),
        "x.y.z"
    );
}

#[test]
#[should_panic(expected = "attempt to redefine field foo")]
fn test_static_field_type_override() {
    Scheme! { foo: Int, foo: Int };
}

#[test]
fn test_field_type_override() {
    let mut builder = Scheme! { foo: Int };

    assert_eq!(
        builder.add_field("foo", Type::Bytes),
        Err(IdentifierRedefinitionError::Field(FieldRedefinitionError(
            "foo".into()
        )))
    );
}

#[test]
fn test_field_lex_indexes() {
    assert_ok!(FieldIndex::lex("0"), FieldIndex::ArrayIndex(0));
    assert_err!(
        FieldIndex::lex("-1"),
        LexErrorKind::ExpectedLiteral("expected positive integer as index"),
        "-1"
    );

    assert_ok!(
        FieldIndex::lex("\"cookies\""),
        FieldIndex::MapKey("cookies".into())
    );
}

#[test]
fn test_scheme_iter_fields() {
    let scheme = &Scheme! {
        x: Bytes,
        x.y.z0: Int,
        is_TCP: Bool,
        map: Map(Bytes)
    }
    .build();

    let mut fields = scheme.fields().collect::<Vec<_>>();
    fields.sort_by(|f1, f2| f1.name().cmp(&f2.name()));

    assert_eq!(
        fields,
        vec![
            scheme.get_field("is_TCP").unwrap(),
            scheme.get_field("map").unwrap(),
            scheme.get_field("x").unwrap(),
            scheme.get_field("x.y.z0").unwrap(),
        ]
    );
}

#[test]
fn test_scheme_json_serialization() {
    let scheme = Scheme! {
        bytes: Bytes,
        int: Int,
        bool: Bool,
        ip: Ip,
        map_of_bytes: Map(Bytes),
        map_of_array_of_bytes: Map(Array(Bytes)),
        array_of_bytes: Array(Bytes),
        array_of_map_of_bytes: Array(Map(Bytes)),
    }
    .build();

    let json = serde_json::to_string(&scheme).unwrap();

    let new_scheme = serde_json::from_str::<Scheme>(&json).unwrap();

    assert_eq!(scheme.inner.scheme_store.field_count(), new_scheme.inner.scheme_store.field_count());
}

#[test]
fn test_nil_not_equal_behavior_true() {
    use crate::{Array, ExecutionContext, Map};

    let scheme = Scheme! {
        arr: Array(Bytes),
        map: Map(Bytes)
    }
    .build();

    let mut ctx = ExecutionContext::<()>::new(&scheme);

    ctx.set_field_value(scheme.get_field("arr").unwrap(), Array::new(Type::Bytes))
        .unwrap();
    ctx.set_field_value(scheme.get_field("map").unwrap(), Map::new(Type::Bytes))
        .unwrap();

    let filter = scheme.parse("arr[0] != \"\"").unwrap().compile();

    assert_eq!(filter.execute(&ctx), Ok(true));

    let filter = scheme.parse("map[\"\"] != \"\"").unwrap().compile();

    assert_eq!(filter.execute(&ctx), Ok(true));

    let mut builder = Scheme! {
        arr: Array(Bytes),
        map: Map(Bytes)
    };

    // Set `nil_not_equal_behavior` to default value of `true`.
    builder.set_nil_not_equal_behavior(true);

    let scheme = builder.build();

    let mut ctx = ExecutionContext::<()>::new(&scheme);

    ctx.set_field_value(scheme.get_field("arr").unwrap(), Array::new(Type::Bytes))
        .unwrap();
    ctx.set_field_value(scheme.get_field("map").unwrap(), Map::new(Type::Bytes))
        .unwrap();

    let filter = scheme.parse("arr[0] != \"\"").unwrap().compile();

    assert_eq!(filter.execute(&ctx), Ok(true));

    let filter = scheme.parse("map[\"\"] != \"\"").unwrap().compile();

    assert_eq!(filter.execute(&ctx), Ok(true));
}

#[test]
fn test_nil_not_equal_behavior_false() {
    use crate::{Array, ExecutionContext, Map};

    let mut builder = Scheme! {
        arr: Array(Bytes),
        map: Map(Bytes)
    };

    builder.set_nil_not_equal_behavior(false);

    let scheme = builder.build();

    let mut ctx = ExecutionContext::<()>::new(&scheme);

    ctx.set_field_value(scheme.get_field("arr").unwrap(), Array::new(Type::Bytes))
        .unwrap();
    ctx.set_field_value(scheme.get_field("map").unwrap(), Map::new(Type::Bytes))
        .unwrap();

    let filter = scheme.parse("arr[0] != \"\"").unwrap().compile();

    assert_eq!(filter.execute(&ctx), Ok(false));

    let filter = scheme.parse("map[\"\"] != \"\"").unwrap().compile();

    assert_eq!(filter.execute(&ctx), Ok(false));
}
