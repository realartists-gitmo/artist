//! Contract-checked values used by dynamically discovered verb packages.
//!
//! These values are intentionally not JSON. Adapters may translate at the
//! process boundary, but the kernel accepts and returns only values checked
//! against the registered Component Model-shaped contract.

use crate::{KernelError, ResourceUri, VerbId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DynamicType {
    Bool,
    S32,
    S64,
    U32,
    U64,
    F64,
    String,
    ResourceUri,
    List(Box<Self>),
    Tuple(Vec<Self>),
    Record(BTreeMap<String, Self>),
    Option(Box<Self>),
    Result {
        ok: Option<Box<Self>>,
        err: Option<Box<Self>>,
    },
    Enum(Vec<String>),
    Variant(BTreeMap<String, Option<Self>>),
    Flags(Vec<String>),
}

impl DynamicType {
    pub fn named(name: &str) -> Result<Self, KernelError> {
        let name = name.trim();
        let primitive = match name {
            "bool" => Some(Self::Bool),
            "s32" => Some(Self::S32),
            "s64" => Some(Self::S64),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "f64" => Some(Self::F64),
            "string" => Some(Self::String),
            "uri" | "resource-uri" => Some(Self::ResourceUri),
            _ => None,
        };
        if let Some(primitive) = primitive {
            return Ok(primitive);
        }
        if let Some(inner) = name
            .strip_prefix("list<")
            .and_then(|value| value.strip_suffix('>'))
        {
            return Ok(Self::List(Box::new(Self::named(inner)?)));
        }
        if let Some(inner) = name
            .strip_prefix("option<")
            .and_then(|value| value.strip_suffix('>'))
        {
            return Ok(Self::Option(Box::new(Self::named(inner)?)));
        }
        if let Some(inner) = name
            .strip_prefix("tuple<")
            .and_then(|value| value.strip_suffix('>'))
        {
            return Ok(Self::Tuple(
                split_type_arguments(inner)
                    .into_iter()
                    .map(Self::named)
                    .collect::<Result<_, _>>()?,
            ));
        }
        Err(KernelError::InvalidRequest {
            message: format!("unknown dynamic contract type: {name}"),
        })
    }
}

fn split_type_arguments(value: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(value[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < value.len() {
        result.push(value[start..].trim());
    }
    result
}

#[derive(Clone, Debug, PartialEq)]
pub enum DynamicValue {
    Bool(bool),
    S32(i32),
    S64(i64),
    U32(u32),
    U64(u64),
    F64(f64),
    String(String),
    ResourceUri(ResourceUri),
    List(Vec<Self>),
    Tuple(Vec<Self>),
    Record(BTreeMap<String, Self>),
    Option(Option<Box<Self>>),
    Result(Result<Box<Self>, Box<Self>>),
    Enum(String),
    Variant(String, Option<Box<Self>>),
    Flags(Vec<String>),
}

impl DynamicValue {
    pub fn validate(&self, ty: &DynamicType) -> Result<(), KernelError> {
        let valid = match (self, ty) {
            (Self::Bool(_), DynamicType::Bool)
            | (Self::S32(_), DynamicType::S32)
            | (Self::S64(_), DynamicType::S64)
            | (Self::U32(_), DynamicType::U32)
            | (Self::U64(_), DynamicType::U64)
            | (Self::F64(_), DynamicType::F64)
            | (Self::String(_), DynamicType::String)
            | (Self::ResourceUri(_), DynamicType::ResourceUri) => true,
            (Self::List(values), DynamicType::List(element)) => {
                values.iter().all(|value| value.validate(element).is_ok())
            }
            (Self::Tuple(values), DynamicType::Tuple(types)) => {
                values.len() == types.len()
                    && values
                        .iter()
                        .zip(types)
                        .all(|(value, ty)| value.validate(ty).is_ok())
            }
            (Self::Record(values), DynamicType::Record(types)) => {
                values.len() == types.len()
                    && types.iter().all(|(name, ty)| {
                        values
                            .get(name)
                            .is_some_and(|value| value.validate(ty).is_ok())
                    })
            }
            (Self::Option(None), DynamicType::Option(_)) => true,
            (Self::Option(Some(value)), DynamicType::Option(ty)) => value.validate(ty).is_ok(),
            (Self::Result(Ok(value)), DynamicType::Result { ok: Some(ty), .. }) => {
                value.validate(ty).is_ok()
            }
            (Self::Result(Err(value)), DynamicType::Result { err: Some(ty), .. }) => {
                value.validate(ty).is_ok()
            }
            (Self::Enum(value), DynamicType::Enum(values)) => values.iter().any(|v| v == value),
            (Self::Variant(name, value), DynamicType::Variant(cases)) => {
                cases.get(name).is_some_and(|ty| match (value, ty) {
                    (None, None) => true,
                    (Some(value), Some(ty)) => value.validate(ty).is_ok(),
                    _ => false,
                })
            }
            (Self::Flags(values), DynamicType::Flags(flags)) => {
                values
                    .iter()
                    .all(|value| flags.iter().any(|flag| flag == value))
                    && values.len()
                        == values
                            .iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(KernelError::InvalidRequest {
                message: format!("value does not satisfy dynamic contract type {ty:?}"),
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicVerbCall {
    pub verb: VerbId,
    pub function: String,
    pub input: DynamicValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicVerbResult {
    pub verb: VerbId,
    pub function: String,
    pub output: DynamicValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_manifest_contract_types() {
        assert_eq!(
            DynamicType::named("list<option<tuple<string,u32>>>").unwrap(),
            DynamicType::List(Box::new(DynamicType::Option(Box::new(DynamicType::Tuple(
                vec![DynamicType::String, DynamicType::U32],
            )))))
        );
    }

    #[test]
    fn validates_nested_component_model_shaped_values() {
        let ty = DynamicType::Record(BTreeMap::from([
            ("name".into(), DynamicType::String),
            (
                "tags".into(),
                DynamicType::List(Box::new(DynamicType::String)),
            ),
        ]));
        let value = DynamicValue::Record(BTreeMap::from([
            ("name".into(), DynamicValue::String("uppercase".into())),
            (
                "tags".into(),
                DynamicValue::List(vec![DynamicValue::String("text".into())]),
            ),
        ]));
        assert!(value.validate(&ty).is_ok());
    }

    #[test]
    fn rejects_wrong_dynamic_value_without_json_coercion() {
        assert!(
            DynamicValue::String("1".into())
                .validate(&DynamicType::U32)
                .is_err()
        );
    }
}
