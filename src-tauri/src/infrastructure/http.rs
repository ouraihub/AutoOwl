use async_trait::async_trait;
use rand::Rng;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// Raw `Set-Cookie` values, in response order.
    ///
    /// QR login is the only flow that needs these: bilibili returns its
    /// credential exclusively through `Set-Cookie`, and netease uses it as the
    /// fallback when the JSON body carries no `cookie` field. Everything else
    /// forwards a stored cookie and ignores this.
    pub set_cookies: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("request timed out")]
    Timeout,
    #[error("request failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get(&self, url: &str) -> Result<HttpResponse, HttpError>;
    async fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError>;
    async fn post_form(
        &self,
        url: &str,
        form: &[(&str, String)],
        headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError>;
    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError>;
}

#[derive(Clone)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(&self, url: &str) -> Result<HttpResponse, HttpError> {
        response(self.client.get(url).send().await).await
    }

    async fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError> {
        let mut request = self.client.get(url);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        response(request.send().await).await
    }

    async fn post_form(
        &self,
        url: &str,
        form: &[(&str, String)],
        headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError> {
        let mut request = self.client.post(url).form(form);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        response(request.send().await).await
    }

    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError> {
        let mut request = self.client.post(url).json(body);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        response(request.send().await).await
    }
}

async fn response(
    result: Result<reqwest::Response, reqwest::Error>,
) -> Result<HttpResponse, HttpError> {
    let response = result.map_err(map_error)?;
    let status = response.status().as_u16();
    // Read headers before `bytes()` consumes the response.
    let set_cookies = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect();
    let body = response.bytes().await.map_err(map_error)?.to_vec();
    Ok(HttpResponse {
        status,
        body,
        set_cookies,
    })
}

fn map_error(error: reqwest::Error) -> HttpError {
    if error.is_timeout() {
        HttpError::Timeout
    } else {
        HttpError::Failed(error.to_string())
    }
}

/// Desktop UA used by every source except migu.
pub const UA_COMMON: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
/// Mobile UA required by migu's endpoints.
pub const UA_MOBILE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 9_1 like Mac OS X) \
AppleWebKit/601.1.46 (KHTML, like Gecko) Version/9.0 Mobile/13B143 Safari/601.1";

/// Per-source User-Agent, Referer and Cookie headers.
///
/// Migu needs a mobile UA, four sources need a Referer, and every source
/// forwards its stored cookie. The qq Referer is exactly `http://y.qq.com`,
/// with no trailing slash; the test below pins it.
pub fn source_headers(source: &str, cookie: &str) -> Vec<(&'static str, String)> {
    let mut headers = Vec::with_capacity(5);
    headers.push((
        "User-Agent",
        if source == "migu" {
            UA_MOBILE
        } else {
            UA_COMMON
        }
        .to_owned(),
    ));
    let referer = match source {
        "migu" => Some("http://music.migu.cn/"),
        "netease" => Some("http://music.163.com/"),
        "bilibili" => Some("https://www.bilibili.com/"),
        "qq" => Some("http://y.qq.com"),
        _ => None,
    };
    if let Some(referer) = referer {
        headers.push(("Referer", referer.to_owned()));
    }
    if !cookie.trim().is_empty() {
        headers.push(("Cookie", cookie.to_owned()));
    }
    if spoofs_client_ip(source) {
        let ip = random_china_ip();
        headers.push(("X-Forwarded-For", ip.clone()));
        headers.push(("X-Real-IP", ip));
    }
    headers
}

/// Sources whose endpoints rate-limit or geo-gate by client IP.
///
/// Verified load-bearing for kugou: `songsearch.kugou.com` throttles a repeat
/// caller to an empty `data.lists` within seconds, which surfaced as an empty
/// search page.
fn spoofs_client_ip(source: &str) -> bool {
    matches!(source, "netease" | "qq" | "kugou" | "kuwo" | "bilibili")
}

/// A random mainland-China address drawn from nine telecom/unicom/mobile
/// prefixes.
pub fn random_china_ip() -> String {
    const PREFIXES: [(u8, u8); 9] = [
        (116, 255),
        (116, 228),
        (218, 192),
        (124, 0),
        (14, 132),
        (183, 14),
        (58, 14),
        (113, 116),
        (120, 230),
    ];
    let mut rng = rand::thread_rng();
    let (first, second) = PREFIXES[rng.gen_range(0..PREFIXES.len())];
    format!(
        "{first}.{second}.{}.{}",
        rng.gen_range(1..=254),
        rng.gen_range(1..=254)
    )
}

/// Headers for adapters that keep their own User-Agent but still need the
/// cookie-when-present plus random-IP treatment of [`source_headers`].
///
/// kugou and kuwo hand-roll their requests because their endpoints require
/// source-specific agents; routing them through here keeps them from sending an
/// empty `Cookie` header and gives them the client IP spoofing.
pub fn adapter_headers(
    user_agent: &'static str,
    referer: Option<&'static str>,
    cookie: &str,
) -> Vec<(&'static str, String)> {
    let mut headers = Vec::with_capacity(4);
    headers.push(("User-Agent", user_agent.to_owned()));
    if let Some(referer) = referer {
        headers.push(("Referer", referer.to_owned()));
    }
    if !cookie.trim().is_empty() {
        headers.push(("Cookie", cookie.to_owned()));
    }
    let ip = random_china_ip();
    headers.push(("X-Forwarded-For", ip.clone()));
    headers.push(("X-Real-IP", ip));
    headers
}

/// A fetched byte span plus the metadata needed for extension detection.
#[derive(Debug, Clone)]
pub struct BytesResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// Total resource length parsed from `Content-Range`.
    pub total: Option<u64>,
    pub content_type: Option<String>,
}

impl BytesResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Range-capable byte fetching used by the download engine.
///
/// Split from [`HttpClient`] because adapters only ever need small JSON calls,
/// while downloads need per-request timeouts and `Content-Range` metadata.
#[async_trait]
pub trait ByteFetcher: Send + Sync {
    /// Fetches `bytes=start-end`; `end` omitted means open ended.
    async fn fetch_range(
        &self,
        url: &str,
        headers: &[(&str, String)],
        start: u64,
        end: Option<u64>,
        timeout: Duration,
    ) -> Result<BytesResponse, HttpError>;

    /// Fetches the whole resource without a Range header.
    async fn fetch_all(
        &self,
        url: &str,
        headers: &[(&str, String)],
        timeout: Duration,
    ) -> Result<BytesResponse, HttpError>;
}

#[async_trait]
impl ByteFetcher for ReqwestHttpClient {
    async fn fetch_range(
        &self,
        url: &str,
        headers: &[(&str, String)],
        start: u64,
        end: Option<u64>,
        timeout: Duration,
    ) -> Result<BytesResponse, HttpError> {
        let range = match end {
            Some(end) => format!("bytes={start}-{end}"),
            None => format!("bytes={start}-"),
        };
        let mut request = self.client.get(url).timeout(timeout).header("Range", range);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        bytes_response(request.send().await).await
    }

    async fn fetch_all(
        &self,
        url: &str,
        headers: &[(&str, String)],
        timeout: Duration,
    ) -> Result<BytesResponse, HttpError> {
        let mut request = self.client.get(url).timeout(timeout);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        bytes_response(request.send().await).await
    }
}

async fn bytes_response(
    result: Result<reqwest::Response, reqwest::Error>,
) -> Result<BytesResponse, HttpError> {
    let response = result.map_err(map_error)?;
    let status = response.status().as_u16();
    let headers = response.headers();
    let total = headers
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range_total);
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty());
    let body = response.bytes().await.map_err(map_error)?.to_vec();
    Ok(BytesResponse {
        status,
        body,
        total,
        content_type,
    })
}

/// Extracts `total` from `bytes <start>-<end>/<total>`; `*` means unknown.
pub fn parse_content_range_total(value: &str) -> Option<u64> {
    let (_, total) = value.rsplit_once('/')?;
    total.trim().parse::<u64>().ok().filter(|value| *value > 0)
}

#[cfg(test)]
pub struct FakeHttpClient {
    pub response: HttpResponse,
}

#[cfg(test)]
#[async_trait]
impl HttpClient for FakeHttpClient {
    async fn get(&self, _url: &str) -> Result<HttpResponse, HttpError> {
        Ok(self.response.clone())
    }

    async fn get_with_headers(
        &self,
        _url: &str,
        _headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError> {
        Ok(self.response.clone())
    }

    async fn post_form(
        &self,
        _url: &str,
        _form: &[(&str, String)],
        _headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError> {
        Ok(self.response.clone())
    }

    async fn post_json(
        &self,
        _url: &str,
        _body: &serde_json::Value,
        _headers: &[(&str, String)],
    ) -> Result<HttpResponse, HttpError> {
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        adapter_headers, parse_content_range_total, random_china_ip, source_headers, UA_COMMON,
        UA_MOBILE,
    };

    fn header<'a>(headers: &'a [(&str, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn migu_uses_mobile_agent_others_desktop() {
        let migu = source_headers("migu", "");
        assert_eq!(header(&migu, "User-Agent"), Some(UA_MOBILE));
        assert_eq!(header(&migu, "Referer"), Some("http://music.migu.cn/"));
        let kuwo = source_headers("kuwo", "");
        assert_eq!(header(&kuwo, "User-Agent"), Some(UA_COMMON));
        assert_eq!(header(&kuwo, "Referer"), None);
    }

    #[test]
    fn qq_referer_has_no_trailing_slash() {
        let headers = source_headers("qq", "");
        assert_eq!(header(&headers, "Referer"), Some("http://y.qq.com"));
    }

    #[test]
    fn blank_cookie_is_not_sent() {
        assert_eq!(header(&source_headers("netease", "   "), "Cookie"), None);
        assert_eq!(
            header(&source_headers("netease", "MUSIC_U=x"), "Cookie"),
            Some("MUSIC_U=x")
        );
    }

    #[test]
    fn ip_spoofing_applies_only_to_gated_sources() {
        for source in ["netease", "qq", "kugou", "kuwo", "bilibili"] {
            let headers = source_headers(source, "");
            let forwarded = header(&headers, "X-Forwarded-For");
            assert!(forwarded.is_some(), "{source} should spoof client IP");
            assert_eq!(forwarded, header(&headers, "X-Real-IP"));
        }
        for source in ["migu", "joox", "apple", "jamendo"] {
            let headers = source_headers(source, "");
            assert_eq!(header(&headers, "X-Forwarded-For"), None);
            assert_eq!(header(&headers, "X-Real-IP"), None);
        }
    }

    #[test]
    fn random_china_ip_stays_in_documented_prefixes() {
        const PREFIXES: [(u8, u8); 9] = [
            (116, 255),
            (116, 228),
            (218, 192),
            (124, 0),
            (14, 132),
            (183, 14),
            (58, 14),
            (113, 116),
            (120, 230),
        ];
        for _ in 0..256 {
            let ip = random_china_ip();
            let octets: Vec<u8> = ip.split('.').map(|part| part.parse().unwrap()).collect();
            assert_eq!(octets.len(), 4, "malformed address {ip}");
            assert!(
                PREFIXES.contains(&(octets[0], octets[1])),
                "unexpected prefix in {ip}"
            );
            assert!((1..=254).contains(&octets[2]) && (1..=254).contains(&octets[3]));
        }
    }

    #[test]
    fn adapter_headers_keep_own_agent_and_drop_blank_cookie() {
        let headers = adapter_headers("Custom/1.0", Some("http://m.kugou.com/"), "  ");
        assert_eq!(header(&headers, "User-Agent"), Some("Custom/1.0"));
        assert_eq!(header(&headers, "Referer"), Some("http://m.kugou.com/"));
        assert_eq!(header(&headers, "Cookie"), None);
        assert!(header(&headers, "X-Forwarded-For").is_some());
        let with_cookie = adapter_headers("Custom/1.0", None, "kg_mid=1");
        assert_eq!(header(&with_cookie, "Cookie"), Some("kg_mid=1"));
        assert_eq!(header(&with_cookie, "Referer"), None);
    }

    #[test]
    fn content_range_total_is_parsed_and_unknown_rejected() {
        assert_eq!(
            parse_content_range_total("bytes 0-3/1048576"),
            Some(1048576)
        );
        assert_eq!(parse_content_range_total("bytes 0-3/*"), None);
        assert_eq!(parse_content_range_total("bytes 0-3"), None);
        assert_eq!(parse_content_range_total("bytes 0-0/0"), None);
    }
}
