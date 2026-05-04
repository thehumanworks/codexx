use prost_types::ListValue;
use prost_types::NullValue;
use prost_types::Struct;
use prost_types::Value;
use prost_types::value;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Map;
use serde_json::Number;

pub fn to_proto_json_payload<T>(payload: &T) -> Result<Value, serde_json::Error>
where
    T: Serialize,
{
    serde_json::to_value(payload).map(serde_json_to_proto_value)
}

pub fn from_proto_json_payload<T>(payload: Value) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    serde_json::from_value(proto_value_to_serde_json(payload))
}

pub fn serde_json_to_proto_value(value: serde_json::Value) -> Value {
    let kind = match value {
        serde_json::Value::Null => value::Kind::NullValue(NullValue::NullValue as i32),
        serde_json::Value::Bool(value) => value::Kind::BoolValue(value),
        serde_json::Value::Number(value) => value::Kind::NumberValue(value.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(value) => value::Kind::StringValue(value),
        serde_json::Value::Array(values) => value::Kind::ListValue(ListValue {
            values: values.into_iter().map(serde_json_to_proto_value).collect(),
        }),
        serde_json::Value::Object(map) => value::Kind::StructValue(Struct {
            fields: map
                .into_iter()
                .map(|(key, value)| (key, serde_json_to_proto_value(value)))
                .collect(),
        }),
    };
    Value { kind: Some(kind) }
}

pub fn proto_value_to_serde_json(value: Value) -> serde_json::Value {
    match value.kind {
        Some(value::Kind::NullValue(_)) | None => serde_json::Value::Null,
        Some(value::Kind::BoolValue(value)) => serde_json::Value::Bool(value),
        Some(value::Kind::NumberValue(value)) => Number::from_f64(value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Some(value::Kind::StringValue(value)) => serde_json::Value::String(value),
        Some(value::Kind::ListValue(value)) => serde_json::Value::Array(
            value
                .values
                .into_iter()
                .map(proto_value_to_serde_json)
                .collect(),
        ),
        Some(value::Kind::StructValue(value)) => {
            let mut map = Map::with_capacity(value.fields.len());
            for (key, value) in value.fields {
                map.insert(key, proto_value_to_serde_json(value));
            }
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn protobuf_json_value_roundtrips_nested_payloads() {
        let original = json!({
            "array": [null, true, 3.25, "text"],
            "object": {
                "nested": "value"
            }
        });

        let value = serde_json_to_proto_value(original.clone());

        assert_eq!(proto_value_to_serde_json(value), original);
    }
}
