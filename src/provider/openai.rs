use super::{AssistantTurn, Provider, ToolCall, ToolSpec, Usage};
use crate::config::ProviderConfig;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::thread;
use std::time::Duration;
use ureq::Agent;

pub struct OpenAiProvider {
    http: Agent,
    base_url: String,
    model: String,
    api_key: String,
    user_agent: String,
    session_id: String,
    temperature: f32,
    max_tokens: u32,
}

impl OpenAiProvider {
    pub fn new(config: &ProviderConfig, api_key: String, session_id: String) -> Self {
        let http = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build();
        Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            api_key,
            user_agent: config.user_agent.clone(),
            session_id,
            temperature: config.temperature,
            max_tokens: config.max_tokens,
        }
    }

    pub fn check_connection(&self) -> Result<String> {
        let url = models_url(&self.base_url);
        let response = self
            .http
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("User-Agent", &self.user_agent)
            .set("x-opencode-session", &self.session_id)
            .call();
        match response {
            Ok(resp) => {
                let value: Value = resp
                    .into_json()
                    .context("respuesta /models no es JSON valido")?;
                let count = value
                    .get("data")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                Ok(format!(
                    "conexion correcta con {} ({} modelos visibles)",
                    self.base_url, count
                ))
            }
            Err(ureq::Error::Status(code, response)) => {
                let body = response.into_string().unwrap_or_default();
                bail!(
                    "el proveedor respondio HTTP {}: {}",
                    code,
                    crate::process::truncate_middle(&body, 800)
                )
            }
            Err(err) => bail!("no se pudo conectar con {}: {}", self.base_url, err),
        }
    }
}

fn models_url(base_url: &str) -> String {
    if let Some(prefix) = base_url.strip_suffix("/openai/v1") {
        format!("{prefix}/v1/models")
    } else {
        format!("{base_url}/models")
    }
}

impl Provider for OpenAiProvider {
    fn complete(&self, messages: &[Value], tools: &[ToolSpec]) -> Result<AssistantTurn> {
        let url = format!("{}/chat/completions", self.base_url);
        let tool_values: Vec<Value> = tools.iter().map(ToolSpec::to_openai).collect();
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "temperature": self.temperature,
            "max_tokens": self.max_tokens,
        });
        if !tool_values.is_empty() {
            body["tools"] = Value::Array(tool_values);
            body["tool_choice"] = json!("auto");
        }

        let mut last_error = String::new();
        for attempt in 0..4 {
            let response = self
                .http
                .post(&url)
                .set("Authorization", &format!("Bearer {}", self.api_key))
                .set("Content-Type", "application/json")
                .set("User-Agent", &self.user_agent)
                .set("x-opencode-session", &self.session_id)
                .send_json(body.clone());

            match response {
                Ok(resp) => {
                    let text = resp
                        .into_string()
                        .context("no se pudo leer la respuesta del proveedor")?;
                    let mut turn = parse_response(&text)?;
                    apply_json_fallback(&mut turn);
                    return Ok(turn);
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let text = resp.into_string().unwrap_or_default();
                    last_error = format!(
                        "HTTP {}: {}",
                        code,
                        crate::process::truncate_middle(&text, 1200)
                    );
                    if attempt < 3 && (code == 429 || code >= 500) {
                        thread::sleep(Duration::from_secs(1 << attempt));
                        continue;
                    }
                    bail!("el proveedor de IA fallo: {last_error}");
                }
                Err(err) => {
                    last_error = err.to_string();
                    if attempt < 3 {
                        thread::sleep(Duration::from_secs(1 << attempt));
                        continue;
                    }
                    bail!("no se pudo contactar con el proveedor de IA: {last_error}");
                }
            }
        }
        bail!("el proveedor de IA fallo: {last_error}")
    }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: Option<Value>,
    tool_calls: Option<Vec<RawToolCall>>,
}

#[derive(Debug, Deserialize)]
struct RawToolCall {
    id: Option<String>,
    function: Option<RawFunction>,
}

#[derive(Debug, Deserialize)]
struct RawFunction {
    name: Option<String>,
    arguments: Option<Value>,
}

fn parse_response(text: &str) -> Result<AssistantTurn> {
    let response: ChatResponse = serde_json::from_str(text).with_context(|| {
        format!(
            "respuesta del proveedor invalida: {}",
            crate::process::truncate_middle(text, 1200)
        )
    })?;
    let choice = response
        .choices
        .into_iter()
        .next()
        .context("el proveedor devolvio una respuesta sin choices")?;
    let content = choice.message.content.and_then(content_to_text);
    let mut tool_calls = Vec::new();
    for (index, raw) in choice
        .message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let Some(function) = raw.function else {
            continue;
        };
        let Some(name) = function.name else {
            continue;
        };
        let arguments = match function.arguments.unwrap_or(Value::Null) {
            Value::String(text) => text,
            Value::Null => "{}".to_string(),
            other => other.to_string(),
        };
        tool_calls.push(ToolCall {
            id: raw.id.unwrap_or_else(|| format!("call_{index}")),
            name,
            arguments,
        });
    }
    Ok(AssistantTurn {
        content,
        tool_calls,
        usage: response.usage,
    })
}

fn apply_json_fallback(turn: &mut AssistantTurn) {
    if !turn.tool_calls.is_empty() {
        return;
    }
    let Some(content) = turn.content.as_deref() else {
        return;
    };
    if let Some(call) = try_json_tool(content) {
        turn.content = None;
        turn.tool_calls.push(call);
    }
}

fn try_json_tool(content: &str) -> Option<ToolCall> {
    let trimmed = content.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    let trimmed = trimmed.strip_suffix("```").unwrap_or(trimmed).trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    let object = value.as_object()?;
    let name = object
        .get("tool")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)?;
    let arguments = object.get("arguments")?;
    let arguments = match arguments {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Some(ToolCall {
        id: format!("json_{}", uuid::Uuid::new_v4()),
        name: name.to_string(),
        arguments,
    })
}

fn content_to_text(value: Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text),
        Value::Array(parts) => {
            let text = parts
                .into_iter()
                .filter_map(|part| {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        Some(text.to_string())
                    } else {
                        part.as_str().map(str::to_string)
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_response() {
        let text = r#"{"choices":[{"message":{"content":"hola"}}],"usage":{"total_tokens":3}}"#;
        let turn = parse_response(text).unwrap();
        assert_eq!(turn.content.as_deref(), Some("hola"));
        assert!(turn.tool_calls.is_empty());
    }

    #[test]
    fn parses_tool_call() {
        let text = r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"run_command","arguments":"{\"command\":\"echo ok\"}"}}]}}]}"#;
        let turn = parse_response(text).unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "run_command");
        assert!(turn.tool_calls[0].arguments.contains("echo ok"));
    }

    #[test]
    fn builds_models_urls() {
        assert_eq!(
            models_url("https://opencode.ai/inference/go/openai/v1"),
            "https://opencode.ai/inference/go/v1/models"
        );
        assert_eq!(
            models_url("http://127.0.0.1:1234/v1"),
            "http://127.0.0.1:1234/v1/models"
        );
    }

    #[test]
    fn parses_json_tool_fallback() {
        let text = r#"{"choices":[{"message":{"content":"{\"tool\":\"run_command\",\"arguments\":{\"command\":\"echo ok\"}}"}}]}"#;
        let mut turn = parse_response(text).unwrap();
        apply_json_fallback(&mut turn);
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "run_command");
        assert!(turn.tool_calls[0].arguments.contains("echo ok"));
    }

    #[test]
    fn ignores_plain_json_answers() {
        let text = r#"{"choices":[{"message":{"content":"{\"nombre\":\"casa\"}"}}]}"#;
        let mut turn = parse_response(text).unwrap();
        apply_json_fallback(&mut turn);
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.content.as_deref(), Some("{\"nombre\":\"casa\"}"));
    }

    #[test]
    fn parses_object_arguments() {
        let text = r#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"x","arguments":{"a":1}}}]}}]}"#;
        let turn = parse_response(text).unwrap();
        assert_eq!(turn.tool_calls[0].arguments, "{\"a\":1}");
    }
}
