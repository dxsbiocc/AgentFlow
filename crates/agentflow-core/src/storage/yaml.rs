use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
use serde::Deserialize;
use serde_yaml_ng::Value;

use super::StorageError;

pub(super) const MAX_YAML_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn parse_yaml<'de, T>(kind: &str, source_text: &'de str) -> Result<T, StorageError>
where
    T: Deserialize<'de>,
{
    parse_yaml_with_byte_cap(kind, source_text, MAX_YAML_DOCUMENT_BYTES)
}

fn parse_yaml_with_byte_cap<'de, T>(
    kind: &str,
    source_text: &'de str,
    max_bytes: usize,
) -> Result<T, StorageError>
where
    T: Deserialize<'de>,
{
    if source_text.len() > max_bytes {
        return Err(StorageError::InvalidInput(format!(
            "{kind} YAML exceeds {max_bytes} byte cap"
        )));
    }
    serde_yaml_ng::from_str(source_text)
        .map_err(|error| StorageError::InvalidInput(format!("cannot parse {kind} YAML: {error}")))
}

pub(super) fn invalid_input_at_field(
    source_text: &str,
    field_name: &str,
    message: impl Into<String>,
) -> StorageError {
    StorageError::InvalidInput(format!(
        "{}{}",
        message.into(),
        field_location_suffix(source_text, field_name)
    ))
}

pub(super) fn with_field_location(
    source_text: &str,
    field_name: &str,
    error: StorageError,
) -> StorageError {
    match error {
        StorageError::InvalidInput(message) => {
            invalid_input_at_field(source_text, field_name, message)
        }
        other => other,
    }
}

pub(super) fn deserialize_default_map<'de, D, V>(
    deserializer: D,
) -> Result<BTreeMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    Option::<BTreeMap<String, V>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

pub(super) fn deserialize_default_vec<'de, D, V>(deserializer: D) -> Result<Vec<V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    Option::<Vec<V>>::deserialize(deserializer).map(Option::unwrap_or_default)
}

pub(super) fn deserialize_optional_scalar_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(None),
        value => scalar_value_to_string(value)
            .map(Some)
            .map_err(de::Error::custom),
    }
}

pub(super) fn deserialize_optional_present_scalar_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(Some(String::new())),
        value => scalar_value_to_string(value)
            .map(Some)
            .map_err(de::Error::custom),
    }
}

pub(super) fn deserialize_bool_like<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value =
        scalar_value_to_string(Value::deserialize(deserializer)?).map_err(de::Error::custom)?;
    match value.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(de::Error::custom(format!(
            "expected boolean true or false, got {value}"
        ))),
    }
}

pub(super) fn deserialize_string_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(Vec::new()),
        Value::Sequence(values) => values
            .into_iter()
            .map(scalar_value_to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(de::Error::custom),
        value => Err(de::Error::custom(format!(
            "expected YAML sequence, got {}",
            value_kind(&value)
        ))),
    }
}

pub(super) fn deserialize_string_vec_or_csv<'de, D>(
    deserializer: D,
) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(Vec::new()),
        Value::Sequence(values) => values
            .into_iter()
            .map(scalar_value_to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(de::Error::custom),
        value => {
            let value = scalar_value_to_string(value).map_err(de::Error::custom)?;
            let trimmed = value.trim();
            if trimmed.is_empty() || trimmed == "[]" {
                return Ok(Vec::new());
            }
            let inner = trimmed
                .strip_prefix('[')
                .and_then(|inner| inner.strip_suffix(']'))
                .unwrap_or(trimmed);
            Ok(inner
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect())
        }
    }
}

pub(super) fn deserialize_required_columns<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(vec![String::new()]),
        Value::Sequence(values) => values
            .into_iter()
            .map(scalar_value_to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(de::Error::custom),
        value => {
            let value = scalar_value_to_string(value).map_err(de::Error::custom)?;
            Ok(value
                .split(',')
                .map(|column| column.trim().to_string())
                .collect())
        }
    }
}

pub(super) fn deserialize_string_map<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(BTreeMap::new()),
        Value::Mapping(mapping) => {
            let mut values = BTreeMap::new();
            for (key, value) in mapping {
                let key = scalar_value_to_string(key).map_err(de::Error::custom)?;
                let value = match value {
                    Value::Null => String::new(),
                    value => scalar_value_to_string(value).map_err(de::Error::custom)?,
                };
                values.insert(key, value);
            }
            Ok(values)
        }
        value => Err(de::Error::custom(format!(
            "expected YAML mapping, got {}",
            value_kind(&value)
        ))),
    }
}

fn field_location_suffix(source_text: &str, field_name: &str) -> String {
    let needle = format!("{field_name}:");
    for (line_index, line) in source_text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&needle) {
            let column = line.len() - trimmed.len() + 1;
            return format!(" at line {}, column {column}", line_index + 1);
        }
    }
    " at line 1, column 1".to_string()
}

fn scalar_value_to_string(value: Value) -> Result<String, String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value),
        Value::Tagged(value) => scalar_value_to_string(value.value),
        value => Err(format!("expected YAML scalar, got {}", value_kind(&value))),
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Sequence(_) => "sequence",
        Value::Mapping(_) => "mapping",
        Value::Tagged(_) => "tagged value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct MinimalYaml {
        name: String,
    }

    #[test]
    fn yaml_parse_rejects_source_over_injected_byte_cap() {
        let source = "name: abc\n";

        let error = parse_yaml_with_byte_cap::<MinimalYaml>("tool", source, 8).unwrap_err();

        assert!(error.to_string().contains("tool YAML exceeds 8 byte cap"));
    }

    #[test]
    fn yaml_parse_accepts_source_at_injected_byte_cap() {
        let source = "name: abc\n";

        let parsed = parse_yaml_with_byte_cap::<MinimalYaml>("tool", source, source.len()).unwrap();

        assert_eq!(
            parsed,
            MinimalYaml {
                name: "abc".to_string()
            }
        );
    }
}
