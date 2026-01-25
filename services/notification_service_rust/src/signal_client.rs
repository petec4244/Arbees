use anyhow::{Context, Result};
use reqwest::Client;
use serde::Serialize;

#[derive(Debug, Clone)]
pub struct SignalClient {
    http: Client,
    base_url: String,
    sender_number: String,
    recipients: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SendRequest<'a> {
    message: &'a str,
    number: &'a str,
    recipients: &'a [String],
}

impl SignalClient {
    pub fn new(base_url: String, sender_number: String, recipients: Vec<String>) -> Self {
        Self {
            http: Client::new(),
            base_url,
            sender_number,
            recipients,
        }
    }

    pub async fn send(&self, message: &str) -> Result<()> {
        let url = format!("{}/v2/send", self.base_url.trim_end_matches('/'));
        let body = SendRequest {
            message,
            number: &self.sender_number,
            recipients: &self.recipients,
        };

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Signal API request failed: {url}"))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("Signal API non-2xx: {status} body={text}");
        }
        Ok(())
    }
}

