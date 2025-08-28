use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

/// File I/O error handling utilities
/// JSON serialization/parsing error handling utilities
///
/// Serialize data to pretty JSON with contextual error handling
pub fn serialize_to_json_with_context<T: Serialize>(data: &T, data_desc: &str) -> Result<String> {
    serde_json::to_string_pretty(data)
        .with_context(|| format!("Failed to serialize {data_desc} to JSON"))
}

/// Parse JSON from string with contextual error handling
pub fn parse_json_with_context<T: DeserializeOwned>(json_str: &str, data_desc: &str) -> Result<T> {
    serde_json::from_str(json_str).with_context(|| format!("Failed to parse {data_desc} from JSON"))
}

/// Parse JSON from reader with contextual error handling
pub fn parse_json_from_reader_with_context<T: DeserializeOwned, R: std::io::Read>(
    reader: R,
    data_desc: &str,
) -> Result<T> {
    serde_json::from_reader(reader)
        .with_context(|| format!("Failed to parse {data_desc} from JSON reader"))
}

/// HTTP request error handling utilities
///
/// Parse HTTP response as JSON with contextual error handling
pub async fn parse_http_response_json<T: DeserializeOwned>(
    response: reqwest::Response,
    api_desc: &str,
) -> Result<T> {
    response
        .json::<T>()
        .await
        .with_context(|| format!("Failed to parse {api_desc} response"))
}

/// Environment variable handling utilities
///
/// Get required environment variable with contextual error handling
pub fn get_required_env_var(var_name: &str) -> Result<String> {
    std::env::var(var_name).with_context(|| format!("{var_name} environment variable not set"))
}

/// Get optional environment variable, returning None if not set or empty
pub fn get_optional_env_var(var_name: &str) -> Option<String> {
    std::env::var(var_name).ok().filter(|v| !v.is_empty())
}

/// Create HTTP client with contextual error handling
pub fn create_http_client_with_context() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .context("Failed to create HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[test]
    fn test_json_serialization_with_context() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let json_str = serialize_to_json_with_context(&data, "test data").unwrap();
        assert!(json_str.contains("\"name\": \"test\""));
        assert!(json_str.contains("\"value\": 42"));

        let parsed_data: TestData = parse_json_with_context(&json_str, "test data").unwrap();
        assert_eq!(parsed_data, data);
    }

    #[test]
    fn test_env_var_handling() {
        // Test required env var (this will likely fail, which is expected)
        let result = get_required_env_var("NONEXISTENT_VAR");
        assert!(result.is_err());

        // Test optional env var
        let result = get_optional_env_var("NONEXISTENT_VAR");
        assert!(result.is_none());

        // Test with a commonly available env var (PATH is usually set on all systems)
        // This avoids needing to set env vars in the test
        if std::env::var("PATH").is_ok() {
            let result = get_optional_env_var("PATH");
            assert!(result.is_some());
        }
    }

    #[test]
    fn test_create_http_client() {
        let client = create_http_client_with_context().unwrap();
        // Just test that we can create a client without error
        assert!(client.get("https://example.com").build().is_ok());
    }
}
