//! One request to an OpenAI-compatible transcription server: a multipart POST
//! of the audio to its `/v1/audio/transcriptions` URL, which answers with JSON
//! holding `text`.

use std::time::Duration;

use reqwest::multipart::{Form, Part};

/// How long the connection to a server may take. A server that is switched
/// off sends nothing back, so this keeps the wait short.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// What one request asks for.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The server's transcription URL, for example
    /// `http://host:8000/v1/audio/transcriptions`.
    pub url: &'a str,
    pub model: &'a str,
    pub language: Option<&'a str>,
    pub prompt: Option<&'a str>,
    /// The limit on the whole request, the transcription included.
    pub timeout: Duration,
}

/// Why a request gave no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrError {
    /// The server's fault: it refused, did not answer in time, answered 5xx,
    /// or answered without text, or it refused the key (401, 403), the path
    /// (404), the request's time (408) or the rate (429). The call is not to
    /// blame.
    Server(String),
    /// The server refused this call (any other 4xx): it counts against the
    /// call.
    Call(String),
    /// The audio could not be read: it counts against the call.
    Audio(String),
}

impl std::fmt::Display for AsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsrError::Server(why) | AsrError::Call(why) | AsrError::Audio(why) => {
                write!(f, "{why}")
            }
        }
    }
}

/// Sends the requests. The API key, when the host has one, goes in the
/// `Authorization` header.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    api_key: Option<String>,
}

impl Client {
    pub fn new(api_key: Option<String>) -> Client {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_default();
        Client { http, api_key }
    }

    /// Transcribes `audio`, the bytes of the file named `file_name`; returns
    /// the text, trimmed (empty when the server heard no speech).
    pub async fn transcribe(
        &self,
        request: &Request<'_>,
        file_name: &str,
        audio: Vec<u8>,
    ) -> Result<String, AsrError> {
        let file = Part::bytes(audio)
            .file_name(file_name.to_string())
            .mime_str("audio/mpeg")
            .map_err(|error| AsrError::Audio(error.to_string()))?;
        let mut form = Form::new()
            .text("model", request.model.to_string())
            .part("file", file);
        if let Some(language) = request.language {
            form = form.text("language", language.to_string());
        }
        if let Some(prompt) = request.prompt {
            form = form.text("prompt", prompt.to_string());
        }
        let mut post = self
            .http
            .post(request.url)
            .timeout(request.timeout)
            .multipart(form);
        if let Some(key) = &self.api_key {
            post = post.bearer_auth(key);
        }
        let answer = post
            .send()
            .await
            .map_err(|error| AsrError::Server(describe(&error)))?;
        let status = answer.status();
        let body = answer
            .bytes()
            .await
            .map_err(|error| AsrError::Server(describe(&error)))?;
        if status.is_client_error() && !servers_fault(status) {
            return Err(AsrError::Call(format!("{status}: {}", excerpt(&body))));
        }
        if !status.is_success() {
            return Err(AsrError::Server(format!("{status}: {}", excerpt(&body))));
        }
        let text = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                AsrError::Server(format!("an answer without text: {}", excerpt(&body)))
            })?;
        Ok(text.trim().to_string())
    }
}

/// A 4xx that says the server or its setup is wrong, not the call: a refused
/// key, a wrong path, a request timeout or a rate limit.
fn servers_fault(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 404 | 408 | 429)
}

/// An error of the connection, in words.
fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "the server did not answer in time".to_string()
    } else if error.is_connect() {
        format!("the server cannot be reached: {error}")
    } else {
        error.to_string()
    }
}

/// The start of an answer's body, for an error message.
fn excerpt(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    match text.char_indices().nth(200) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text.to_string(),
    }
}
