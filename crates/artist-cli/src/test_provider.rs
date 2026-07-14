use anyhow::{Context, Result, bail};
use llm_provider::SavedProvider;
use reqwest::Client;
use serde_json::{Value, json};
use std::time::Duration;

pub async fn test(provider: &SavedProvider) -> Result<()> {
    let model = provider
        .model
        .as_deref()
        .context("no model selected; run `artist model` first")?;
    let client = Client::builder().timeout(Duration::from_secs(45)).build()?;
    let endpoint = provider.base_url.join("responses")?;
    let mut body = json!({"model": model, "input": "Reply with exactly OK.", "stream": false});
    if let Some(effort) = &provider.reasoning_effort {
        body["reasoning"] = json!({"effort": effort});
    }
    let response = client
        .post(endpoint)
        .headers(provider.request_auth()?.headers)
        .header("originator", "artist")
        .header(
            reqwest::header::USER_AGENT,
            concat!("artist/", env!("CARGO_PKG_VERSION")),
        )
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!("provider returned {status}: {}", sanitized(&text));
    }
    if !response_contains_ok(&text) {
        bail!("provider responded successfully but did not reply OK");
    }
    Ok(())
}

fn response_contains_ok(text: &str) -> bool {
    if text.lines().any(|line| {
        line.strip_prefix("data: ")
            .is_some_and(|data| data.contains("OK"))
    }) {
        return true;
    }
    serde_json::from_str::<Value>(text).is_ok_and(|value| {
        value
            .pointer("/output_text")
            .and_then(Value::as_str)
            .is_some_and(has_ok)
            || value.get("output").is_some_and(output_contains_ok)
    })
}
fn output_contains_ok(value: &Value) -> bool {
    match value {
        Value::String(text) => has_ok(text),
        Value::Array(values) => values.iter().any(output_contains_ok),
        Value::Object(fields) => fields
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "content" | "text" | "output_text"))
            .any(|(_, value)| output_contains_ok(value)),
        _ => false,
    }
}
fn has_ok(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("ok")
}
fn sanitized(value: &str) -> String {
    value
        .chars()
        .take(500)
        .collect::<String>()
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::response_contains_ok;
    #[test]
    fn checks_output_instead_of_echoed_input() {
        assert!(response_contains_ok(
            r#"{"output":[{"content":[{"text":"OK"}]}]}"#
        ));
        assert!(!response_contains_ok(
            r#"{"input":"Reply with exactly OK.","output_text":"no"}"#
        ));
    }
}
