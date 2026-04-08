#![forbid(unsafe_code)]

use anyhow::{Context, Result, anyhow, bail};
use nostrweet_core::{BlossomPort, BlossomUrl, HttpUrl, MediaAsset};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, instrument, warn};

const DEFAULT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MAX_ATTEMPTS: usize = 3;
const DEFAULT_RETRY_DELAY_MS: u64 = 250;

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_attempts: usize,
    pub base_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            base_delay: Duration::from_millis(DEFAULT_RETRY_DELAY_MS),
        }
    }
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

trait BlossomHttp: Send + Sync + std::fmt::Debug {
    fn head<'a>(
        &'a self,
        url: &'a str,
        headers: HeaderMap,
    ) -> BoxFuture<'a, Result<BlossomHttpResponse>>;
    fn put<'a>(
        &'a self,
        url: &'a str,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'a, Result<BlossomHttpResponse>>;
}

#[derive(Debug, Clone)]
struct BlossomHttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ReqwestBlossomHttp {
    client: Client,
}

impl ReqwestBlossomHttp {
    fn new(client: Client) -> Self {
        Self { client }
    }
}

impl BlossomHttp for ReqwestBlossomHttp {
    fn head<'a>(
        &'a self,
        url: &'a str,
        headers: HeaderMap,
    ) -> BoxFuture<'a, Result<BlossomHttpResponse>> {
        Box::pin(async move {
            let response = self
                .client
                .head(url)
                .headers(headers)
                .send()
                .await
                .with_context(|| format!("HEAD {url} failed"))?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = response.bytes().await?.to_vec();
            Ok(BlossomHttpResponse {
                status,
                headers,
                body,
            })
        })
    }

    fn put<'a>(
        &'a self,
        url: &'a str,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'a, Result<BlossomHttpResponse>> {
        Box::pin(async move {
            let response = self
                .client
                .put(url)
                .headers(headers)
                .body(body)
                .send()
                .await
                .with_context(|| format!("PUT {url} failed"))?;
            let status = response.status();
            let headers = response.headers().clone();
            let body = response.bytes().await?.to_vec();
            Ok(BlossomHttpResponse {
                status,
                headers,
                body,
            })
        })
    }
}

#[derive(Debug, Clone)]
pub struct BlossomClient {
    servers: Vec<BlossomUrl>,
    http: Arc<dyn BlossomHttp>,
    auth_header: Option<String>,
    retry: RetryConfig,
}

impl BlossomClient {
    pub fn new(servers: Vec<BlossomUrl>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .context("Failed to build Blossom HTTP client")?;
        let http = Arc::new(ReqwestBlossomHttp::new(client));
        Ok(Self {
            servers,
            http,
            auth_header: None,
            retry: RetryConfig::default(),
        })
    }

    pub fn with_auth_header(mut self, header: impl Into<String>) -> Self {
        self.auth_header = Some(header.into());
        self
    }

    pub fn with_retry_config(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    fn upload_url(server: &BlossomUrl) -> Result<String> {
        let base = server.as_str();
        let normalized = if base.ends_with('/') {
            base.to_string()
        } else {
            format!("{base}/")
        };
        let url = format!("{normalized}upload");
        HttpUrl::parse(&url).context("Invalid Blossom upload URL")?;
        Ok(url)
    }

    async fn upload_single(&self, asset: &MediaAsset) -> Result<HttpUrl> {
        let sha256 = sha256_hex(&asset.bytes);
        let content_len = asset.bytes.len();

        for server in &self.servers {
            let upload_url = Self::upload_url(server)?;
            let mut headers = HeaderMap::new();
            insert_header(&mut headers, "x-content-length", content_len.to_string())?;
            insert_header(&mut headers, "x-content-type", &asset.content_type)?;
            insert_header(&mut headers, "x-sha-256", &sha256)?;
            let head_resp = self.http.head(&upload_url, headers).await?;

            if !head_resp.status.is_success() {
                warn!(
                    "Blossom HEAD request failed with status {}",
                    head_resp.status
                );
                continue;
            }

            let invoice = head_resp
                .headers
                .get("X-Lightning")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);

            match self
                .upload_with_retries(&upload_url, asset, invoice.as_deref())
                .await
            {
                Ok(url) => return Ok(url),
                Err(err) => {
                    warn!("Blossom upload failed for {upload_url}: {err}");
                }
            }
        }

        bail!("Failed to upload media asset {}", asset.name);
    }

    async fn upload_with_retries(
        &self,
        upload_url: &str,
        asset: &MediaAsset,
        invoice: Option<&str>,
    ) -> Result<HttpUrl> {
        let max_attempts = self.retry.max_attempts.max(1);
        let mut attempt = 0;

        loop {
            attempt += 1;
            debug!(
                "Uploading media asset {} (attempt {}/{})",
                asset.name, attempt, max_attempts
            );

            let mut headers = HeaderMap::new();
            insert_header(&mut headers, "content-type", &asset.content_type)?;
            if let Some(auth_header) = &self.auth_header {
                insert_header(&mut headers, "authorization", auth_header)?;
            }
            if let Some(invoice) = invoice {
                insert_header(&mut headers, "x-lightning", invoice)?;
            }

            let response = self
                .http
                .put(upload_url, headers, asset.bytes.clone())
                .await?;

            if response.status == StatusCode::TOO_MANY_REQUESTS && attempt < max_attempts {
                let delay = retry_delay(&response.headers, self.retry.base_delay);
                warn!("Blossom server rate-limited; retrying in {:?}", delay);
                tokio::time::sleep(delay).await;
                continue;
            }

            if !response.status.is_success() {
                bail!("Blossom upload failed with status {}", response.status);
            }

            let json: Value = serde_json::from_slice(&response.body)
                .context("Failed to parse Blossom response JSON")?;
            let url =
                extract_url(&json).ok_or_else(|| anyhow!("Blossom response missing URL field"))?;
            return HttpUrl::parse(&url).context("Invalid Blossom response URL");
        }
    }
}

impl BlossomPort for BlossomClient {
    #[instrument(name = "blossom.upload_media", skip(self, assets))]
    async fn upload_media(&self, assets: &[MediaAsset]) -> Result<Vec<HttpUrl>> {
        if self.servers.is_empty() {
            bail!("No Blossom servers provided for media upload");
        }

        let mut uploaded = Vec::with_capacity(assets.len());
        for asset in assets {
            let url = self.upload_single(asset).await?;
            uploaded.push(url);
        }

        Ok(uploaded)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn extract_url(json: &Value) -> Option<String> {
    if let Some(url) = json.get("url").and_then(|value| value.as_str()) {
        return Some(url.to_string());
    }

    let tags = json
        .get("nip94_event")
        .and_then(|event| event.get("tags"))
        .and_then(|tags| tags.as_array())?;
    for tag in tags {
        let arr = tag.as_array()?;
        if arr.first()?.as_str()? == "url" {
            return arr.get(1)?.as_str().map(str::to_string);
        }
    }

    None
}

fn retry_delay(headers: &HeaderMap, fallback: Duration) -> Duration {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: impl AsRef<str>) -> Result<()> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .with_context(|| format!("Invalid header name {name}"))?;
    let value = HeaderValue::from_str(value.as_ref())
        .with_context(|| format!("Invalid header value for {name}"))?;
    headers.insert(name, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    #[derive(Debug, Clone)]
    struct MockResponse {
        status: StatusCode,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    #[derive(Debug, Default)]
    struct StateData {
        head_requests: Vec<RecordedRequest>,
        put_requests: Vec<RecordedRequest>,
        head_invoice: Option<String>,
        put_responses: VecDeque<MockResponse>,
    }

    #[derive(Debug, Default)]
    struct MockHttpState {
        next_id: usize,
        servers: HashMap<String, Arc<Mutex<StateData>>>,
    }

    #[derive(Debug, Clone, Default)]
    struct MockHttp {
        state: Arc<Mutex<MockHttpState>>,
    }

    struct MockBlossomServer {
        state: Arc<Mutex<StateData>>,
        base_url: String,
    }

    impl MockHttp {
        fn server(&self) -> MockBlossomServer {
            let mut state = self.state.lock().expect("lock mock state");
            state.next_id += 1;
            let base_url = format!("http://mock-{}.test", state.next_id);
            let server_state = Arc::new(Mutex::new(StateData::default()));
            state
                .servers
                .insert(base_url.clone(), Arc::clone(&server_state));
            MockBlossomServer {
                base_url,
                state: server_state,
            }
        }

        fn server_state(&self, url: &str) -> Result<Arc<Mutex<StateData>>> {
            let base = base_url(url)?;
            let state = self.state.lock().expect("lock mock state");
            state
                .servers
                .get(&base)
                .cloned()
                .ok_or_else(|| anyhow!("Unknown mock server for {base}"))
        }
    }

    impl BlossomHttp for MockHttp {
        fn head<'a>(
            &'a self,
            url: &'a str,
            headers: HeaderMap,
        ) -> BoxFuture<'a, Result<BlossomHttpResponse>> {
            Box::pin(async move {
                let state = self.server_state(url)?;
                let mut state = state.lock().expect("lock server state");
                state
                    .head_requests
                    .push(record_request(headers, Vec::new()));

                let mut response_headers = HeaderMap::new();
                if let Some(invoice) = &state.head_invoice {
                    response_headers.insert(
                        HeaderName::from_static("x-lightning"),
                        HeaderValue::from_str(invoice).expect("valid invoice header"),
                    );
                }

                Ok(BlossomHttpResponse {
                    status: StatusCode::OK,
                    headers: response_headers,
                    body: Vec::new(),
                })
            })
        }

        fn put<'a>(
            &'a self,
            url: &'a str,
            headers: HeaderMap,
            body: Vec<u8>,
        ) -> BoxFuture<'a, Result<BlossomHttpResponse>> {
            Box::pin(async move {
                let state = self.server_state(url)?;
                let mut state = state.lock().expect("lock server state");
                state.put_requests.push(record_request(headers, body));

                let response = state.put_responses.pop_front().unwrap_or(MockResponse {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    headers: Vec::new(),
                    body: Vec::new(),
                });

                Ok(response.into_http_response())
            })
        }
    }

    impl MockResponse {
        fn into_http_response(self) -> BlossomHttpResponse {
            let mut headers = HeaderMap::new();
            for (name, value) in self.headers {
                let name = name.to_ascii_lowercase();
                let header_name =
                    HeaderName::from_bytes(name.as_bytes()).expect("valid header name");
                let header_value = HeaderValue::from_str(&value).expect("valid header value");
                headers.insert(header_name, header_value);
            }
            BlossomHttpResponse {
                status: self.status,
                headers,
                body: self.body,
            }
        }
    }

    impl MockBlossomServer {
        fn blossom_url(&self) -> BlossomUrl {
            BlossomUrl::parse(&self.base_url).expect("valid base url")
        }

        fn set_invoice(&self, invoice: &str) {
            let mut state = self.state.lock().expect("lock state");
            state.head_invoice = Some(invoice.to_string());
        }

        fn queue_response(&self, response: MockResponse) {
            let mut state = self.state.lock().expect("lock state");
            state.put_responses.push_back(response);
        }

        fn state(&self) -> Arc<Mutex<StateData>> {
            self.state.clone()
        }
    }

    fn record_request(headers: HeaderMap, body: Vec<u8>) -> RecordedRequest {
        let mut header_map = HashMap::new();
        for (name, value) in headers.iter() {
            if let Ok(value) = value.to_str() {
                header_map.insert(name.as_str().to_ascii_lowercase(), value.to_string());
            }
        }

        RecordedRequest {
            headers: header_map,
            body,
        }
    }

    fn base_url(url: &str) -> Result<String> {
        let url = reqwest::Url::parse(url)?;
        let host = url.host_str().ok_or_else(|| anyhow!("missing host"))?;
        let mut base = format!("{}://{}", url.scheme(), host);
        if let Some(port) = url.port() {
            base.push_str(&format!(":{port}"));
        }
        Ok(base)
    }

    #[tokio::test]
    async fn upload_media_sends_headers_and_parses_url() -> Result<()> {
        let mock_http = MockHttp::default();
        let server = mock_http.server();
        server.set_invoice("lnbc1test");
        server.queue_response(MockResponse {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: br#"{ "url": "https://cdn.example.com/file.jpg" }"#.to_vec(),
        });

        let client = BlossomClient {
            servers: vec![server.blossom_url()],
            http: Arc::new(mock_http),
            auth_header: Some("Nostr test".to_string()),
            retry: RetryConfig::default(),
        };
        let asset = MediaAsset {
            name: "photo.jpg".to_string(),
            content_type: "image/jpeg".to_string(),
            bytes: b"hello".to_vec(),
        };

        let uploaded = client.upload_media(std::slice::from_ref(&asset)).await?;
        assert_eq!(uploaded.len(), 1);
        assert_eq!(uploaded[0].as_str(), "https://cdn.example.com/file.jpg");

        let state = server.state();
        let state = state.lock().expect("lock state");
        assert_eq!(state.head_requests.len(), 1);
        let head = &state.head_requests[0];
        assert_eq!(head.headers.get("x-content-length"), Some(&"5".to_string()));
        assert_eq!(
            head.headers.get("x-content-type"),
            Some(&"image/jpeg".to_string())
        );
        assert_eq!(
            head.headers.get("x-sha-256"),
            Some(&sha256_hex(&asset.bytes))
        );

        assert_eq!(state.put_requests.len(), 1);
        let put = &state.put_requests[0];
        assert_eq!(
            put.headers.get("content-type"),
            Some(&"image/jpeg".to_string())
        );
        assert_eq!(
            put.headers.get("authorization"),
            Some(&"Nostr test".to_string())
        );
        assert_eq!(
            put.headers.get("x-lightning"),
            Some(&"lnbc1test".to_string())
        );
        assert_eq!(put.body, asset.bytes);
        Ok(())
    }

    #[tokio::test]
    async fn upload_media_retries_on_rate_limit() -> Result<()> {
        let mock_http = MockHttp::default();
        let server = mock_http.server();
        server.queue_response(MockResponse {
            status: StatusCode::TOO_MANY_REQUESTS,
            headers: vec![("Retry-After".to_string(), "0".to_string())],
            body: Vec::new(),
        });
        server.queue_response(MockResponse {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: br#"{ "url": "https://cdn.example.com/retry.jpg" }"#.to_vec(),
        });

        let client = BlossomClient {
            servers: vec![server.blossom_url()],
            http: Arc::new(mock_http),
            auth_header: None,
            retry: RetryConfig {
                max_attempts: 2,
                base_delay: Duration::from_millis(0),
            },
        };
        let asset = MediaAsset {
            name: "retry.jpg".to_string(),
            content_type: "image/jpeg".to_string(),
            bytes: b"retry".to_vec(),
        };

        let uploaded = client.upload_media(&[asset]).await?;
        assert_eq!(uploaded[0].as_str(), "https://cdn.example.com/retry.jpg");

        let state = server.state();
        let state = state.lock().expect("lock state");
        assert_eq!(state.put_requests.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn upload_media_falls_back_to_next_server() -> Result<()> {
        let mock_http = MockHttp::default();
        let first = mock_http.server();
        first.queue_response(MockResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            headers: Vec::new(),
            body: Vec::new(),
        });

        let second = mock_http.server();
        second.queue_response(MockResponse {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: br#"{ "url": "https://cdn.example.com/fallback.jpg" }"#.to_vec(),
        });

        let client = BlossomClient {
            servers: vec![first.blossom_url(), second.blossom_url()],
            http: Arc::new(mock_http),
            auth_header: None,
            retry: RetryConfig::default(),
        };
        let asset = MediaAsset {
            name: "fallback.jpg".to_string(),
            content_type: "image/jpeg".to_string(),
            bytes: b"fallback".to_vec(),
        };

        let uploaded = client.upload_media(&[asset]).await?;
        assert_eq!(uploaded[0].as_str(), "https://cdn.example.com/fallback.jpg");

        let first_state = first.state();
        let first_state = first_state.lock().expect("lock state");
        assert_eq!(first_state.put_requests.len(), 1);

        let second_state = second.state();
        let second_state = second_state.lock().expect("lock state");
        assert_eq!(second_state.put_requests.len(), 1);
        Ok(())
    }
}
