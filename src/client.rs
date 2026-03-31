//! GitLab API HTTP client.
//!
//! Reusable async client with connection pooling and structured error handling.

use reqwest::{Client, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::error;

use crate::config::GitLabInstance;

/// GitLab API client for a single instance.
#[derive(Clone)]
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
    pub async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T, ApiError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.get(&url).query(params).send().await?;
        self.handle_response(resp).await
    }

    /// GET request returning raw JSON Value.
    pub async fn get_json(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, ApiError> {
        self.get(path, params).await
    }

    /// POST request with JSON body.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, ApiError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.post(&url).json(body).send().await?;
        self.handle_response(resp).await
    }

    /// PUT request with JSON body.
    pub async fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, ApiError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.put(&url).json(body).send().await?;
        self.handle_response(resp).await
    }

    /// DELETE request.
    pub async fn delete(&self, path: &str) -> Result<(), ApiError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.delete(&url).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(self.extract_error(resp).await)
        }
    }

    /// Extract error details from a non-success response.
    async fn extract_error(&self, resp: Response) -> ApiError {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .or_else(|| v.get("error"))
                    .map(|m| m.to_string())
            })
            .unwrap_or_else(|| body.chars().take(200).collect());

        error!(
            status = status.as_u16(),
            path = "",
            error_type = "gitlab_api",
            "GitLab API error: {message}"
        );

        ApiError::GitLab { status, message }
    }

    /// Handle response: check status, deserialize.
    async fn handle_response<T: DeserializeOwned>(&self, resp: Response) -> Result<T, ApiError> {
        if resp.status().is_success() {
            let body = resp.text().await?;
            if body.is_empty() {
                // Return default for empty responses
                serde_json::from_str("null").map_err(|e| ApiError::Parse(e.to_string()))
            } else {
                serde_json::from_str(&body).map_err(|e| ApiError::Parse(e.to_string()))
            }
        } else {
            Err(self.extract_error(resp).await)
        }
    }
}

/// API error types.
#[derive(Debug)]
pub enum ApiError {
    GitLab { status: StatusCode, message: String },
    Http(reqwest::Error),
    Parse(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::GitLab { status, message } => {
                write!(f, "GitLab API error ({status}): {message}")
            }
            ApiError::Http(e) => write!(f, "HTTP error: {e}"),
            ApiError::Parse(e) => write!(f, "Parse error: {e}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        ApiError::Http(e)
    }
}
