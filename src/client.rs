//! GitLab API HTTP client.
//!
//! Reusable async client with connection pooling, response caching, and structured error handling.

use reqwest::{Client, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{error, warn};

use crate::config::GitLabInstance;
use crate::error::{Error, Result};

/// Maximum number of retry attempts for rate-limited (429) responses.
const MAX_RETRIES: u32 = 3;

/// Default TTL for cached responses (seconds).
const DEFAULT_CACHE_TTL: u64 = 60;

/// GitLab API client for a single instance.
#[derive(Clone)]
pub struct GitLabClient {
    pub name: String,
    base_url: String,
    http: Client,
    cache: Arc<Mutex<HashMap<String, (Instant, String)>>>,
}

impl GitLabClient {
    pub fn new(instance: &GitLabInstance) -> Result<Self> {
        let token_header = instance.token.parse().map_err(|_| {
            Error::Config(format!("Invalid token for instance '{}': not a valid HTTP header value", instance.name))
        })?;

        let http = Client::builder()
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert("PRIVATE-TOKEN", token_header);
                h
            })
            .timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(10)
            .build()
            .map_err(|e| Error::Config(format!("Failed to build HTTP client for '{}': {e}", instance.name)))?;

        Ok(Self {
            name: instance.name.clone(),
            base_url: format!("{}/api/v4", instance.url),
            http,
            cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// GET request, returning deserialized JSON. Retries on HTTP 429.
    pub async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut attempt = 0u32;
        loop {
            let resp = self.http.get(&url).query(params).send().await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                let retry_after = Self::parse_retry_after(&resp);
                attempt += 1;
                warn!(
                    attempt,
                    retry_after_secs = retry_after,
                    path,
                    "Rate limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                continue;
            }
            return self.handle_response(resp).await;
        }
    }

    /// GET request with TTL-based caching. Use for frequently repeated lookups.
    pub async fn get_cached<T: DeserializeOwned>(
        &self,
        cache_key: &str,
        path: &str,
        params: &[(&str, &str)],
        ttl_secs: u64,
    ) -> Result<T> {
        // Check cache
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((stored_at, json_str)) = cache.get(cache_key) {
                if stored_at.elapsed().as_secs() < ttl_secs {
                    return Ok(serde_json::from_str(json_str)?);
                }
            }
        }

        // Cache miss — fetch from API
        let url = format!("{}{}", self.base_url, path);
        let mut attempt = 0u32;
        let body: String = loop {
            let resp = self.http.get(&url).query(params).send().await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                let retry_after = Self::parse_retry_after(&resp);
                attempt += 1;
                warn!(
                    attempt,
                    retry_after_secs = retry_after,
                    path,
                    "Rate limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                continue;
            }
            if resp.status().is_success() {
                break resp.text().await?;
            } else {
                return Err(self.extract_error(resp).await);
            }
        };

        // Store in cache
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            // Evict expired entries if cache grows large
            if cache.len() > 500 {
                cache.retain(|_, (stored_at, _)| stored_at.elapsed().as_secs() < DEFAULT_CACHE_TTL);
            }
            cache.insert(cache_key.to_string(), (Instant::now(), body.clone()));
        }

        if body.is_empty() {
            Ok(serde_json::from_str("null")?)
        } else {
            Ok(serde_json::from_str(&body)?)
        }
    }

    /// POST request with JSON body. Retries on HTTP 429.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut attempt = 0u32;
        loop {
            let resp = self.http.post(&url).json(body).send().await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                let retry_after = Self::parse_retry_after(&resp);
                attempt += 1;
                warn!(
                    attempt,
                    retry_after_secs = retry_after,
                    path,
                    "Rate limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                continue;
            }
            return self.handle_response(resp).await;
        }
    }

    /// PUT request with JSON body. Retries on HTTP 429.
    pub async fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut attempt = 0u32;
        loop {
            let resp = self.http.put(&url).json(body).send().await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                let retry_after = Self::parse_retry_after(&resp);
                attempt += 1;
                warn!(
                    attempt,
                    retry_after_secs = retry_after,
                    path,
                    "Rate limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                continue;
            }
            return self.handle_response(resp).await;
        }
    }

    /// DELETE request. Retries on HTTP 429.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let mut attempt = 0u32;
        loop {
            let resp = self.http.delete(&url).send().await?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                let retry_after = Self::parse_retry_after(&resp);
                attempt += 1;
                warn!(
                    attempt,
                    retry_after_secs = retry_after,
                    path,
                    "Rate limited (429), retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                continue;
            }
            if resp.status().is_success() {
                return Ok(());
            } else {
                return Err(self.extract_error(resp).await);
            }
        }
    }

    /// Parse Retry-After header from a 429 response, defaulting to exponential backoff.
    fn parse_retry_after(resp: &Response) -> u64 {
        resp.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1)
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
                if body.len() > 200 { body.chars().take(200).collect() } else { body }
            });

        error!(
            status = status.as_u16(),
            error_type = "gitlab_api",
            "GitLab API error: {message}"
        );

        Error::GitLab { status, message }
    }

    /// Fetch all pages of a paginated endpoint.
    pub async fn get_all_pages<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
        max_pages: usize,
    ) -> Result<Vec<T>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut p: Vec<(&str, &str)> = params.to_vec();
            let page_str = page.to_string();
            p.push(("page", &page_str));
            p.push(("per_page", "100"));
            let batch: Vec<T> = self.get(path, &p).await?;
            if batch.is_empty() || page as usize >= max_pages {
                all.extend(batch);
                break;
            }
            all.extend(batch);
            page += 1;
        }
        Ok(all)
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
