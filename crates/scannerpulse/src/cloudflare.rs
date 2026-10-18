//! Cloudflare's DNS API, for the TXT record that proves a name to Let's
//! Encrypt (DNS-01). The token needs only "Zone, DNS, Edit" on the zones of
//! the site's names; it is never shown, in a message or a log line.

use serde_json::Value;

const API: &str = "https://api.cloudflare.com/client/v4";

pub struct Cloudflare {
    http: reqwest::Client,
    api: String,
    token: String,
}

impl Cloudflare {
    pub fn new(token: &str) -> Cloudflare {
        Cloudflare::with_api(API, token)
    }

    /// At the address SCANNERPULSE_CLOUDFLARE_API names (a stand-in, for a
    /// trial), else Cloudflare's own.
    pub fn from_env(token: &str) -> Cloudflare {
        match std::env::var("SCANNERPULSE_CLOUDFLARE_API") {
            Ok(api) if !api.is_empty() => Cloudflare::with_api(&api, token),
            _ => Cloudflare::new(token),
        }
    }

    /// Another address for the API (a stand-in, in the tests).
    pub fn with_api(api: &str, token: &str) -> Cloudflare {
        Cloudflare {
            http: reqwest::Client::new(),
            api: api.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    /// The id of the zone that holds `name`: the longest of its suffixes
    /// that is a zone the token reaches.
    pub async fn zone(&self, name: &str) -> Result<String, String> {
        let mut rest = name;
        loop {
            let found = self
                .call(
                    self.http
                        .get(format!("{}/zones", self.api))
                        .query(&[("name", rest)]),
                )
                .await?;
            if let Some(id) = found.get(0).and_then(|z| z["id"].as_str()) {
                return Ok(id.to_string());
            }
            match rest.split_once('.') {
                Some((_, parent)) if parent.contains('.') => rest = parent,
                _ => {
                    return Err(format!(
                        "no Cloudflare zone holds {name} (or the token may not reach it)"
                    ));
                }
            }
        }
    }

    /// Adds the TXT record `name` = `value` in `zone`; gives its id.
    pub async fn add_txt(&self, zone: &str, name: &str, value: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "type": "TXT",
            "name": name,
            "content": value,
            "ttl": 60,
        });
        let made = self
            .call(
                self.http
                    .post(format!("{}/zones/{zone}/dns_records", self.api))
                    .header("content-type", "application/json")
                    .body(body.to_string()),
            )
            .await?;
        made["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "Cloudflare gave no id for the record it made".into())
    }

    pub async fn remove(&self, zone: &str, record: &str) -> Result<(), String> {
        self.call(
            self.http
                .delete(format!("{}/zones/{zone}/dns_records/{record}", self.api)),
        )
        .await
        .map(drop)
    }

    /// Sends `request` with the token; gives the answer's `result`, or
    /// Cloudflare's own words when it refused.
    async fn call(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let answer = request
            .bearer_auth(&self.token)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| format!("Cloudflare did not answer: {}", error.without_url()))?;
        let status = answer.status();
        let body: Value = answer
            .bytes()
            .await
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        if body["success"] == Value::Bool(true) {
            return Ok(body["result"].clone());
        }
        let said: Vec<String> = body["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["message"].as_str().map(String::from))
            .collect();
        Err(if said.is_empty() {
            format!("Cloudflare refused ({status})")
        } else {
            format!("Cloudflare refused: {}", said.join("; "))
        })
    }
}
