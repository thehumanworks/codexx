//! Strict config validation built on top of serde's ignored-field tracking.

use crate::diagnostics::ConfigDiagnosticSource;
use crate::diagnostics::ConfigError;
use crate::diagnostics::config_error_from_toml_for_source;
use crate::diagnostics::default_range;
use crate::diagnostics::span_for_config_path;
use crate::diagnostics::span_for_toml_key_path;
use crate::diagnostics::text_range_from_span;
use serde::de::DeserializeOwned;
use std::path::Path;
use toml::Value as TomlValue;

pub fn config_error_from_ignored_toml_fields<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    contents: &str,
) -> Option<ConfigError> {
    let source = ConfigDiagnosticSource::Path(path.as_ref());
    let value = match toml::from_str::<TomlValue>(contents) {
        Ok(value) => value,
        Err(err) => return Some(config_error_from_toml_for_source(source, contents, err)),
    };

    config_error_from_ignored_toml_value_fields_for_source::<T>(source, contents, value)
}

pub(crate) fn config_error_from_ignored_toml_value_fields<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    contents: &str,
    value: TomlValue,
) -> Option<ConfigError> {
    config_error_from_ignored_toml_value_fields_for_source::<T>(
        ConfigDiagnosticSource::Path(path.as_ref()),
        contents,
        value,
    )
}

pub(crate) fn config_error_from_ignored_toml_value_fields_for_source_name<T: DeserializeOwned>(
    source_name: &str,
    contents: &str,
    value: TomlValue,
) -> Option<ConfigError> {
    config_error_from_ignored_toml_value_fields_for_source::<T>(
        ConfigDiagnosticSource::DisplayName(source_name),
        contents,
        value,
    )
}

fn config_error_from_ignored_toml_value_fields_for_source<T: DeserializeOwned>(
    source: ConfigDiagnosticSource<'_>,
    contents: &str,
    value: TomlValue,
) -> Option<ConfigError> {
    let mut ignored_paths = Vec::new();
    let mut ignored_callback = |ignored_path: serde_ignored::Path<'_>| {
        let path_segments = ignored_path_segments(&ignored_path);
        if !path_segments.is_empty() {
            ignored_paths.push(path_segments);
        }
    };
    let deserializer = serde_ignored::Deserializer::new(value, &mut ignored_callback);
    let result: Result<T, _> = serde_path_to_error::deserialize(deserializer);

    match result {
        Ok(_) => unknown_field_error_from_paths(source, contents, ignored_paths),
        Err(err) => {
            let path_hint = err.path().clone();
            let toml_err = err.into_inner();
            let range = span_for_config_path(contents, &path_hint)
                .or_else(|| toml_err.span())
                .map(|span| text_range_from_span(contents, span))
                .unwrap_or_else(default_range);
            Some(ConfigError::new(
                source.to_path_buf(),
                range,
                toml_err.message(),
            ))
        }
    }
}

pub(crate) fn ignored_toml_value_field<T: DeserializeOwned>(value: TomlValue) -> Option<String> {
    let mut ignored_paths = Vec::new();
    let result: Result<T, _> = serde_ignored::deserialize(value, |ignored_path| {
        let path_segments = ignored_path_segments(&ignored_path);
        if !path_segments.is_empty() {
            ignored_paths.push(path_segments);
        }
    });
    if result.is_err() {
        return None;
    }

    ignored_paths
        .into_iter()
        .next()
        .map(|path_segments| path_segments.join("."))
}

fn unknown_field_error_from_paths(
    source: ConfigDiagnosticSource<'_>,
    contents: &str,
    ignored_paths: Vec<Vec<String>>,
) -> Option<ConfigError> {
    let path_segments = ignored_paths.into_iter().next()?;
    let ignored_path = path_segments.join(".");
    let range = span_for_toml_key_path(contents, &path_segments)
        .map(|span| text_range_from_span(contents, span))
        .unwrap_or_else(default_range);
    Some(ConfigError::new(
        source.to_path_buf(),
        range,
        format!("unknown configuration field `{ignored_path}`"),
    ))
}

fn ignored_path_segments(path: &serde_ignored::Path<'_>) -> Vec<String> {
    let mut segments = Vec::new();
    push_ignored_path_segments(path, &mut segments);
    segments
}

fn push_ignored_path_segments(path: &serde_ignored::Path<'_>, segments: &mut Vec<String>) {
    match path {
        serde_ignored::Path::Root => {}
        serde_ignored::Path::Seq { parent, index } => {
            push_ignored_path_segments(parent, segments);
            segments.push(index.to_string());
        }
        serde_ignored::Path::Map { parent, key } => {
            push_ignored_path_segments(parent, segments);
            segments.push(key.clone());
        }
        serde_ignored::Path::Some { parent }
        | serde_ignored::Path::NewtypeStruct { parent }
        | serde_ignored::Path::NewtypeVariant { parent } => {
            push_ignored_path_segments(parent, segments);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_toml::ConfigToml;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    #[test]
    fn ignored_toml_field_errors_accept_non_file_source_names() {
        let source_name = "com.openai.codex:config_toml_base64";
        let contents = "model = \"gpt-5\"\nunknown_key = true";

        let value = toml::from_str::<TomlValue>(contents).expect("valid TOML");
        let error = config_error_from_ignored_toml_value_fields_for_source_name::<ConfigToml>(
            source_name,
            contents,
            value,
        )
        .expect("unknown field error");

        assert_eq!(error.path, PathBuf::from(source_name));
        assert_eq!(error.range.start.line, 2);
        assert_eq!(error.range.start.column, 1);
        assert_eq!(error.message, "unknown configuration field `unknown_key`");
    }

    #[test]
    fn type_errors_take_precedence_over_ignored_fields() {
        let path = Path::new("/tmp/config.toml");
        let contents = "model_context_window = \"wide\"\nunknown_key = true";

        let error = config_error_from_ignored_toml_fields::<ConfigToml>(path, contents)
            .expect("type error");

        assert_eq!(error.path, path);
        assert_eq!(error.range.start.line, 1);
        assert_eq!(error.range.start.column, 24);
        assert_eq!(error.message, "invalid type: string \"wide\", expected i64");
    }
}
