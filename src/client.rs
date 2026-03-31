//! GitLab API HTTP client.
//!
//! Reusable async client with connection pooling and structured error handling.

use reqwest::{Client, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::error;

use crate::config::GitLabInstance;
use crate::error::{Error, Result};

/// GitLab API client for a single instance.
#[derive(Clone)]
#[allow(dead_code)]
pub struct GitLabClient {
    pub name: String,
    base_url: String,
    http: Client,
}

impl GitLabClient {
    pub fn new(instance: &GitLabInstance) -> Self {
        let http = Client::builder()
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    "PRIVATE-TOKEN",
                    instance.token.parse().expect("invalid token header"),
                );
                h
            })
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(10)
            .build()
            .expect("failed to build HTTP client");

        Self {
            name: instance.name.clone(),
            base_url: format!("{}/api/v4", instance.url),
            http,
        }
    }

    /// GET request, returning deserialized JSON.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.get(&url).query(params).send().await?;
        self.handle_response(resp).await
    }

    /// POST request with JSON body.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.post(&url).json(body).send().await?;
        self.handle_response(resp).await
    }

    /// PUT request with JSON body.
    pub async fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.put(&url).json(body).send().await?;
        self.handle_response(resp).await
    }

    /// DELETE request.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.delete(&url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(self.extract_error(resp).await)
        }
    }

    async fn extract_error(&self, resp: Response) -> Error {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .or_else(|| v.get("error"))
                    .map(|m| m.to_string())
            })
            .unwrap_or_else(|| {
                if body.len() > 200 { body[..200].to_string() } else { body }
            });

        error!(
            status = status.as_u16(),
            error_type = "gitlab_api",
            "GitLab API error: {message}"
        );

        Error::GitLab { status, message }
    }

    async fn handle_response<T: DeserializeOwned>(&self, resp: Response) -> Result<T> {
        if resp.status().is_success() {
            let body = resp.text().await?;
            if body.is_empty() {
                Ok(serde_json::from_str("null")?)
            } else {
                Ok(serde_json::from_str(&body)?)
            }
        } else {
            Err(self.extract_error(resp).await)
        }
    }
}
