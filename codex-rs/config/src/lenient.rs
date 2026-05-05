use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::Schema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use toml::Value as TomlValue;

#[derive(Debug, Clone, PartialEq)]
pub enum Lenient<T> {
    Valid(T),
    Invalid(TomlValue),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LenientInput<T> {
    Valid(T),
    Invalid(TomlValue),
}

impl<T> Lenient<T> {
    pub fn as_valid(&self) -> Option<&T> {
        match self {
            Self::Valid(value) => Some(value),
            Self::Invalid(_) => None,
        }
    }

    pub fn into_valid(self) -> Option<T> {
        match self {
            Self::Valid(value) => Some(value),
            Self::Invalid(_) => None,
        }
    }

    pub fn invalid_value(&self) -> Option<&TomlValue> {
        match self {
            Self::Valid(_) => None,
            Self::Invalid(value) => Some(value),
        }
    }
}

impl<T> From<T> for Lenient<T> {
    fn from(value: T) -> Self {
        Self::Valid(value)
    }
}

impl<'de, T> Deserialize<'de> for Lenient<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match LenientInput::<T>::deserialize(deserializer)? {
            LenientInput::Valid(value) => Self::Valid(value),
            LenientInput::Invalid(value) => Self::Invalid(value),
        })
    }
}

impl<T> Serialize for Lenient<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Valid(value) => value.serialize(serializer),
            Self::Invalid(value) => value.serialize(serializer),
        }
    }
}

impl<T> JsonSchema for Lenient<T>
where
    T: JsonSchema,
{
    fn schema_name() -> String {
        T::schema_name()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}

pub fn invalid_config_warnings<T>(field_path: &str, value: &Option<Lenient<T>>) -> Option<String> {
    value.as_ref().and_then(|value| {
        value
            .invalid_value()
            .map(|invalid| format!("Ignoring invalid config value at {field_path}: {invalid}"))
    })
}
