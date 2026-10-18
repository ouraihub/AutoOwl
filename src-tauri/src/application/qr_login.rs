//! QR-code login for netease, qq and bilibili.
//!
//! Each source has its own two-step protocol: `create` obtains a key plus a
//! scannable image, and `check` polls until the code is confirmed and yields a
//! cookie string.
//!
//! netease and bilibili hand back a *login URL*, so the URL is rendered to a PNG
//! here — the UI never needs a QR encoder. qq's `ptqrshow` already returns PNG
//! bytes, which are base64'd unchanged.

use base64::Engine;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::auth::QrStatus;
use crate::infrastructure::http::{HttpClient, HttpResponse, UA_COMMON};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginSession {
    pub source: String,
    /// Opaque token the client echoes back when polling.
    pub key: String,
    /// `data:image/png;base64,...` — always populated, so the UI never needs a QR encoder.
    pub image_url: String,
    /// Unix seconds.
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QrLoginResult {
    pub source: String,
    pub status: QrStatus,
    pub message: String,
    /// Present only when `status == Confirmed`.
    pub cookie: Option<String>,
}

/// Starts a QR login and returns the key to poll plus a renderable PNG.
pub async fn create(client: &dyn HttpClient, source: &str) -> Result<QrLoginSession, String> {
    match source {
        "netease" => netease_create(client).await,
        "bilibili" => bilibili_create(client).await,
        "qq" => qq_create(client).await,
        other => Err(format!("{other} 不支持扫码登录")),
    }
}

/// Polls a QR login session created by [`create`].
pub async fn check(
    client: &dyn HttpClient,
    source: &str,
    key: &str,
) -> Result<QrLoginResult, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("扫码登录凭据为空".to_owned());
    }
    match source {
        "netease" => netease_check(client, key).await,
        "bilibili" => bilibili_check(client, key).await,
        "qq" => qq_check(client, key).await,
        other => Err(format!("{other} 不支持扫码登录")),
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}

/// Collapses raw `Set-Cookie` header values into a `name=value; name=value` string.
///
/// Only the crumb before the first `;` matters,
/// blank names are dropped, later values win, and the output is sorted by name so
/// repeated polls produce a stable credential.
fn join_set_cookies(set_cookies: &[String]) -> String {
    let mut jar: BTreeMap<&str, &str> = BTreeMap::new();
    for entry in set_cookies {
        let crumb = entry.split(';').next().unwrap_or_default().trim();
        let Some((name, value)) = crumb.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        jar.insert(name, value.trim());
    }
    join_cookie_jar(&jar)
}

fn join_cookie_jar<V: AsRef<str>>(jar: &BTreeMap<impl AsRef<str> + Ord, V>) -> String {
    let mut parts = Vec::with_capacity(jar.len());
    for (name, value) in jar {
        parts.push(format!("{}={}", name.as_ref(), value.as_ref()));
    }
    parts.join("; ")
}

/// Merges `Set-Cookie` crumbs into a running jar, ignoring blank names/values.
fn absorb_set_cookies(jar: &mut BTreeMap<String, String>, set_cookies: &[String]) {
    for entry in set_cookies {
        let crumb = entry.split(';').next().unwrap_or_default().trim();
        let Some((name, value)) = crumb.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        jar.insert(name.to_owned(), value.to_owned());
    }
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a str>) -> &'a str {
    values
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// `application/x-www-form-urlencoded` escaping for a single component.
fn query_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn query_unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or_default();
                match u8::from_str_radix(hex, 16) {
                    Ok(decoded) => {
                        out.push(decoded);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode_query(params: &[(&str, String)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", query_escape(key), query_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn query_value(query: &str, key: &str) -> String {
    let query = query.split_once('?').map_or(query, |(_, rest)| rest);
    let query = query.split('#').next().unwrap_or_default();
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if query_unescape(name) == key {
            return query_unescape(value);
        }
    }
    String::new()
}

fn ok_body(response: HttpResponse, what: &str) -> Result<Vec<u8>, String> {
    if response.status != 200 {
        return Err(format!("{what} 请求失败: HTTP {}", response.status));
    }
    Ok(response.body)
}

fn parse_json(body: &[u8], what: &str) -> Result<serde_json::Value, String> {
    serde_json::from_slice(body).map_err(|error| format!("{what} 响应解析失败: {error}"))
}

fn data_url_png(bytes: &[u8]) -> String {
    let mut url = String::from("data:image/png;base64,");
    url.push_str(&base64::engine::general_purpose::STANDARD.encode(bytes));
    url
}

/// Renders a login URL as a scannable PNG data URL.
fn qr_data_url(payload: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(payload.as_bytes())
        .map_err(|error| format!("二维码生成失败: {error}"))?;
    let modules: Vec<bool> = code
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    let width = code.width();
    Ok(data_url_png(&png::gray_modules(&modules, width, 6, 4)))
}

/// Minimal grayscale PNG writer.
///
/// Only exists to avoid dragging the `image` crate (and its decoder tree) in for
/// a two-colour bitmap. The deflate stream uses stored blocks, which is fine at
/// these sizes: a version-10 QR at scale 6 is well under 300 KiB.
mod png {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    /// Expands a row-major `dark` module grid into a PNG, applying `scale`
    /// pixels per module and a `quiet` module border.
    pub fn gray_modules(modules: &[bool], width: usize, scale: usize, quiet: usize) -> Vec<u8> {
        let side = (width + quiet * 2) * scale;
        // Filter byte 0 (None) per scanline, then one gray byte per pixel.
        let mut raw = Vec::with_capacity(side * (side + 1));
        for y in 0..side {
            raw.push(0);
            let module_y = y / scale;
            for x in 0..side {
                let module_x = x / scale;
                let dark = module_y >= quiet
                    && module_x >= quiet
                    && module_y < quiet + width
                    && module_x < quiet + width
                    && modules
                        .get((module_y - quiet) * width + (module_x - quiet))
                        .copied()
                        .unwrap_or(false);
                raw.push(if dark { 0x00 } else { 0xFF });
            }
        }

        let side = side as u32;
        let mut out = Vec::with_capacity(raw.len() + 128);
        out.extend_from_slice(&SIGNATURE);
        let mut header = Vec::with_capacity(13);
        header.extend_from_slice(&side.to_be_bytes());
        header.extend_from_slice(&side.to_be_bytes());
        header.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grayscale, no interlace
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"IDAT", &zlib_stored(&raw));
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut crc = crc32(0xFFFF_FFFF, kind);
        crc = crc32(crc, body);
        out.extend_from_slice(&(crc ^ 0xFFFF_FFFF).to_be_bytes());
    }

    /// zlib container around deflate "stored" blocks.
    fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 11);
        out.extend_from_slice(&[0x78, 0x01]); // deflate, 32K window, fastest
        let mut chunks = raw.chunks(0xFFFF).peekable();
        if raw.is_empty() {
            out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
        }
        while let Some(block) = chunks.next() {
            let len = block.len() as u16;
            out.push(u8::from(chunks.peek().is_none()));
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(block);
        }
        out.extend_from_slice(&adler32(raw).to_be_bytes());
        out
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut low, mut high) = (1u32, 0u32);
        for byte in data {
            low = (low + u32::from(*byte)) % 65521;
            high = (high + low) % 65521;
        }
        (high << 16) | low
    }

    fn crc32(seed: u32, data: &[u8]) -> u32 {
        let mut crc = seed;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let carry = crc & 1;
                crc >>= 1;
                if carry != 0 {
                    crc ^= 0xEDB8_8320;
                }
            }
        }
        crc
    }
}

// ---------------------------------------------------------------- netease

const NETEASE_UNIKEY_API: &str = "https://interface.music.163.com/api/login/qrcode/unikey";
const NETEASE_CHECK_API: &str = "https://interface.music.163.com/api/login/qrcode/client/login";
const NETEASE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; WOW64) AppleWebKit/537.36 (KHTML, like \
Gecko) Safari/537.36 Chrome/91.0.4472.164 NeteaseMusicDesktop/3.0.18.203152";

fn netease_headers() -> Vec<(&'static str, String)> {
    let ip = crate::infrastructure::http::random_china_ip();
    vec![
        ("User-Agent", NETEASE_UA.to_owned()),
        ("Referer", "http://music.163.com/".to_owned()),
        ("X-Forwarded-For", ip.clone()),
        ("X-Real-IP", ip),
    ]
}

async fn netease_create(client: &dyn HttpClient) -> Result<QrLoginSession, String> {
    let response = client
        .post_form(
            NETEASE_UNIKEY_API,
            &[("type", "3".to_owned())],
            &netease_headers(),
        )
        .await
        .map_err(|error| format!("网易云二维码申请失败: {error}"))?;
    let json = parse_json(&ok_body(response, "网易云二维码申请")?, "网易云二维码申请")?;
    let code = json
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    let unikey = json
        .get("unikey")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    if code != 200 || unikey.is_empty() {
        return Err(format!("网易云二维码申请失败: code={code}"));
    }
    let login_url = format!(
        "https://music.163.com/login?codekey={}",
        query_escape(unikey)
    );
    Ok(QrLoginSession {
        source: "netease".to_owned(),
        key: unikey.to_owned(),
        image_url: qr_data_url(&login_url)?,
        expires_at: now_unix() + 300,
    })
}

async fn netease_check(client: &dyn HttpClient, key: &str) -> Result<QrLoginResult, String> {
    let response = client
        .post_form(
            NETEASE_CHECK_API,
            &[("key", key.to_owned()), ("type", "3".to_owned())],
            &netease_headers(),
        )
        .await
        .map_err(|error| format!("网易云扫码状态查询失败: {error}"))?;
    let set_cookies = response.set_cookies.clone();
    let json = parse_json(
        &ok_body(response, "网易云扫码状态查询")?,
        "网易云扫码状态查询",
    )?;
    let code = json
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    let status = netease_status(code);
    let message = json
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    // The JSON `cookie` field is authoritative; `Set-Cookie` is the fallback.
    let cookie = matches!(status, QrStatus::Confirmed).then(|| {
        let inline = json
            .get("cookie")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        if inline.is_empty() {
            join_set_cookies(&set_cookies)
        } else {
            inline.to_owned()
        }
    });
    Ok(QrLoginResult {
        source: "netease".to_owned(),
        status,
        message: if message.is_empty() {
            netease_message(code).to_owned()
        } else {
            message
        },
        cookie,
    })
}

fn netease_status(code: i64) -> QrStatus {
    match code {
        800 => QrStatus::Expired,
        801 => QrStatus::Pending,
        802 => QrStatus::Scanned,
        803 => QrStatus::Confirmed,
        _ => QrStatus::Failed,
    }
}

fn netease_message(code: i64) -> &'static str {
    match code {
        800 => "二维码已过期，请重新获取",
        801 => "等待扫码",
        802 => "已扫码，请在手机上确认",
        803 => "登录成功",
        _ => "扫码登录失败",
    }
}

// --------------------------------------------------------------- bilibili

const BILIBILI_GENERATE_API: &str =
    "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";
const BILIBILI_POLL_API: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";

fn bilibili_headers() -> Vec<(&'static str, String)> {
    let ip = crate::infrastructure::http::random_china_ip();
    vec![
        ("User-Agent", UA_COMMON.to_owned()),
        ("Referer", "https://www.bilibili.com/".to_owned()),
        ("X-Forwarded-For", ip.clone()),
        ("X-Real-IP", ip),
    ]
}

async fn bilibili_create(client: &dyn HttpClient) -> Result<QrLoginSession, String> {
    let response = client
        .get_with_headers(BILIBILI_GENERATE_API, &bilibili_headers())
        .await
        .map_err(|error| format!("哔哩哔哩二维码申请失败: {error}"))?;
    let json = parse_json(
        &ok_body(response, "哔哩哔哩二维码申请")?,
        "哔哩哔哩二维码申请",
    )?;
    let code = json
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    let data = json.get("data");
    let qrcode_key = data
        .and_then(|data| data.get("qrcode_key"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    let login_url = data
        .and_then(|data| data.get("url"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    if code != 0 || qrcode_key.is_empty() || login_url.is_empty() {
        let message = json
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        return Err(format!(
            "哔哩哔哩二维码申请失败: code={code} message={message}"
        ));
    }
    Ok(QrLoginSession {
        source: "bilibili".to_owned(),
        key: qrcode_key.to_owned(),
        image_url: qr_data_url(login_url)?,
        expires_at: now_unix() + 180,
    })
}

async fn bilibili_check(client: &dyn HttpClient, key: &str) -> Result<QrLoginResult, String> {
    let url = format!(
        "{BILIBILI_POLL_API}?{}",
        encode_query(&[("qrcode_key", key.to_owned())])
    );
    let response = client
        .get_with_headers(&url, &bilibili_headers())
        .await
        .map_err(|error| format!("哔哩哔哩扫码状态查询失败: {error}"))?;
    let set_cookies = response.set_cookies.clone();
    let json = parse_json(
        &ok_body(response, "哔哩哔哩扫码状态查询")?,
        "哔哩哔哩扫码状态查询",
    )?;
    let data = json.get("data");
    // Deliberately -1, not 0: a body without `data.code` is a
    // malformed response, and 0 would report it as a successful login.
    let code = data
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1);
    let status = bilibili_status(code);
    let inner = data
        .and_then(|data| data.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let outer = json
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let message = first_non_empty([inner, outer, bilibili_message(code)]).to_owned();
    // bilibili delivers the credential exclusively through `Set-Cookie`.
    let cookie = matches!(status, QrStatus::Confirmed).then(|| join_set_cookies(&set_cookies));
    Ok(QrLoginResult {
        source: "bilibili".to_owned(),
        status,
        message,
        cookie,
    })
}

fn bilibili_status(code: i64) -> QrStatus {
    match code {
        0 => QrStatus::Confirmed,
        86038 => QrStatus::Expired,
        86090 => QrStatus::Scanned,
        86101 => QrStatus::Pending,
        _ => QrStatus::Failed,
    }
}

fn bilibili_message(code: i64) -> &'static str {
    match code {
        0 => "登录成功",
        86038 => "二维码已过期，请重新获取",
        86090 => "已扫码，请在手机上确认",
        86101 => "等待扫码",
        _ => "扫码登录失败",
    }
}

// --------------------------------------------------------------------- qq

const QQ_LOGIN_PAGE_API: &str = "https://xui.ptlogin2.qq.com/cgi-bin/xlogin";
const QQ_QRSHOW_API: &str = "https://xui.ptlogin2.qq.com/ssl/ptqrshow";
const QQ_QRCHECK_API: &str = "https://xui.ptlogin2.qq.com/ssl/ptqrlogin";
const QQ_OAUTH_LOGIN_JUMP: &str = "https://graph.qq.com/oauth2.0/login_jump";
const QQ_OAUTH_AUTHORIZE: &str = "https://graph.qq.com/oauth2.0/authorize";
const QQ_MUSIC_LOGIN_API: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const QQ_OAUTH_CLIENT_ID: &str = "100497308";
const QQ_OAUTH_SCOPE: &str = "get_user_info,get_app_friends";
const QQ_APPID: &str = "716027609";
const QQ_PTLOGIN_REFERER: &str = "https://xui.ptlogin2.qq.com/";

/// A djb2-style rolling hash over the qrsig, masked to 31 bits.
///
/// The multiply-add runs on 64 bits and must wrap silently —
/// a 30-character qrsig overflows long before the mask is applied.
fn hash33(value: &str) -> i64 {
    let mut hash: i64 = 0;
    for ch in value.chars() {
        hash = hash
            .wrapping_add(hash.wrapping_shl(5))
            .wrapping_add(ch as i64);
    }
    hash & 0x7fff_ffff
}

/// Same hash seeded with 5381, used for the OAuth `g_tk` parameter.
fn gtk33(value: &str) -> i64 {
    let mut hash: i64 = 5381;
    for ch in value.chars() {
        hash = hash
            .wrapping_add(hash.wrapping_shl(5))
            .wrapping_add(ch as i64);
    }
    hash & 0x7fff_ffff
}

/// Extracts `code`, `message` and `redirectURL` from a `ptuiCB('...')` callback.
///
/// Every single-quoted group in order, then
/// groups 0, 2 and 4. Fewer than five groups means the body is not a callback,
/// so it is surfaced verbatim as the message.
fn parse_qq_check(raw: &str) -> (String, String, String) {
    let mut groups: Vec<&str> = Vec::with_capacity(6);
    let mut rest = raw;
    while let Some(start) = rest.find('\'') {
        let after = &rest[start + 1..];
        match after.find('\'') {
            Some(end) => {
                groups.push(&after[..end]);
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    if groups.len() >= 5 {
        (
            groups[0].to_owned(),
            groups[4].to_owned(),
            groups[2].to_owned(),
        )
    } else {
        (String::new(), raw.to_owned(), String::new())
    }
}

fn qq_status(code: &str) -> QrStatus {
    match code {
        "0" => QrStatus::Confirmed,
        "65" => QrStatus::Expired,
        "66" => QrStatus::Pending,
        "67" => QrStatus::Scanned,
        _ => QrStatus::Failed,
    }
}

fn qq_headers(jar: &BTreeMap<String, String>) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("User-Agent", UA_COMMON.to_owned()),
        ("Accept", "*/*".to_owned()),
        ("Referer", QQ_PTLOGIN_REFERER.to_owned()),
    ];
    let cookie = join_cookie_jar(jar);
    if !cookie.is_empty() {
        headers.push(("Cookie", cookie));
    }
    headers
}

async fn qq_create(client: &dyn HttpClient) -> Result<QrLoginSession, String> {
    let mut jar: BTreeMap<String, String> = BTreeMap::new();

    // Warm-up: xlogin seeds `pt_login_sig`, which ptqrlogin later demands.
    let page = format!(
        "{QQ_LOGIN_PAGE_API}?{}",
        encode_query(&[
            ("appid", QQ_APPID.to_owned()),
            ("daid", "383".to_owned()),
            ("style", "33".to_owned()),
            ("login_text", "登录".to_owned()),
            ("hide_title_bar", "1".to_owned()),
            ("hide_border", "1".to_owned()),
            ("target", "self".to_owned()),
            ("s_url", QQ_OAUTH_LOGIN_JUMP.to_owned()),
            ("pt_3rd_aid", QQ_OAUTH_CLIENT_ID.to_owned()),
            (
                "pt_feedback_link",
                format!(
                    "https://support.qq.com/products/77942?customInfo=.appid{QQ_OAUTH_CLIENT_ID}"
                ),
            ),
            ("theme", "2".to_owned()),
            ("verify_theme", String::new()),
        ])
    );
    if let Ok(response) = client.get_with_headers(&page, &qq_headers(&jar)).await {
        absorb_set_cookies(&mut jar, &response.set_cookies);
    }

    let show = format!(
        "{QQ_QRSHOW_API}?{}",
        encode_query(&[
            ("appid", QQ_APPID.to_owned()),
            ("e", "2".to_owned()),
            ("l", "M".to_owned()),
            ("s", "3".to_owned()),
            ("d", "72".to_owned()),
            ("v", "4".to_owned()),
            ("t", format!("{:.6}", now_millis() as f64 / 1000.0)),
            ("daid", "383".to_owned()),
            ("pt_3rd_aid", QQ_OAUTH_CLIENT_ID.to_owned()),
            ("u1", QQ_OAUTH_LOGIN_JUMP.to_owned()),
        ])
    );
    let response = client
        .get_with_headers(&show, &qq_headers(&jar))
        .await
        .map_err(|error| format!("QQ 二维码申请失败: {error}"))?;
    absorb_set_cookies(&mut jar, &response.set_cookies);
    // `ptqrshow` answers with the PNG itself — no QR encoding needed here.
    let image = ok_body(response, "QQ 二维码申请")?;
    let qrsig = jar.get("qrsig").cloned().unwrap_or_default();
    if qrsig.is_empty() {
        return Err("QQ 二维码申请失败: 缺少 qrsig".to_owned());
    }

    // The key carries the whole ptlogin cookie state: polling needs the cookies
    // minted here, and the caller treats the blob as opaque.
    let mut key = vec![("qrsig", qrsig)];
    if let Some(login_sig) = jar.get("pt_login_sig").filter(|value| !value.is_empty()) {
        key.push(("login_sig", login_sig.clone()));
    }
    let state = encode_qq_state(&jar);
    if !state.is_empty() {
        key.push(("session", state));
    }
    Ok(QrLoginSession {
        source: "qq".to_owned(),
        key: encode_query(&key),
        image_url: data_url_png(&image),
        expires_at: now_unix() + 120,
    })
}

async fn qq_check(client: &dyn HttpClient, key: &str) -> Result<QrLoginResult, String> {
    let qrsig = query_value(key, "qrsig");
    if qrsig.is_empty() {
        return Err("QQ 扫码凭据缺少 qrsig".to_owned());
    }
    let login_sig = query_value(key, "login_sig");
    let mut jar = decode_qq_state(&query_value(key, "session"));
    jar.insert("qrsig".to_owned(), qrsig.clone());
    if !login_sig.is_empty() {
        jar.insert("pt_login_sig".to_owned(), login_sig.clone());
    }

    let poll = format!(
        "{QQ_QRCHECK_API}?{}",
        encode_query(&[
            ("u1", QQ_OAUTH_LOGIN_JUMP.to_owned()),
            ("ptqrtoken", hash33(&qrsig).to_string()),
            ("ptredirect", "0".to_owned()),
            ("h", "1".to_owned()),
            ("t", "1".to_owned()),
            ("g", "1".to_owned()),
            ("from_ui", "1".to_owned()),
            ("ptlang", "2052".to_owned()),
            ("action", format!("0-0-{}", now_millis())),
            ("js_ver", "26071711".to_owned()),
            ("js_type", "1".to_owned()),
            ("login_sig", login_sig),
            ("pt_uistyle", "40".to_owned()),
            ("aid", QQ_APPID.to_owned()),
            ("daid", "383".to_owned()),
            ("pt_3rd_aid", QQ_OAUTH_CLIENT_ID.to_owned()),
            ("pt_js_version", "c1987b96".to_owned()),
        ])
    );
    let response = client
        .get_with_headers(&poll, &qq_headers(&jar))
        .await
        .map_err(|error| format!("QQ 扫码状态查询失败: {error}"))?;
    absorb_set_cookies(&mut jar, &response.set_cookies);
    let raw = String::from_utf8_lossy(&response.body).into_owned();
    let (code, message, redirect_url) = parse_qq_check(&raw);
    let status = qq_status(&code);
    if !matches!(status, QrStatus::Confirmed) {
        return Ok(QrLoginResult {
            source: "qq".to_owned(),
            status,
            message: first_non_empty([message.as_str(), qq_message(&code)]).to_owned(),
            cookie: None,
        });
    }
    if redirect_url.trim().is_empty() {
        return Ok(QrLoginResult {
            source: "qq".to_owned(),
            status: QrStatus::Failed,
            message: "QQ 登录缺少跳转地址".to_owned(),
            cookie: None,
        });
    }
    // ptlogin only yields p_skey/skey. The usable music credential (`qm_keyst`)
    // comes from the OAuth handshake that follows.
    match qq_finish_music_login(&mut jar, redirect_url.trim()).await {
        Ok(cookie) => Ok(QrLoginResult {
            source: "qq".to_owned(),
            status: QrStatus::Confirmed,
            message: first_non_empty([message.as_str(), "登录成功"]).to_owned(),
            cookie: Some(cookie),
        }),
        Err(error) => Ok(QrLoginResult {
            source: "qq".to_owned(),
            status: QrStatus::Failed,
            message: error,
            cookie: None,
        }),
    }
}

fn qq_message(code: &str) -> &'static str {
    match code {
        "0" => "登录成功",
        "65" => "二维码已过期，请重新获取",
        "66" => "等待扫码",
        "67" => "已扫码，请在手机上确认",
        _ => "扫码登录失败",
    }
}

fn encode_qq_state(jar: &BTreeMap<String, String>) -> String {
    let raw = join_cookie_jar(jar);
    if raw.is_empty() {
        String::new()
    } else {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
    }
}

fn decode_qq_state(encoded: &str) -> BTreeMap<String, String> {
    let encoded = encoded.trim();
    if encoded.is_empty() {
        return BTreeMap::new();
    }
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        return BTreeMap::new();
    };
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let mut jar = BTreeMap::new();
    for part in raw.split(';') {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if !name.is_empty() && !value.is_empty() {
            jar.insert(name.to_owned(), value.to_owned());
        }
    }
    jar
}

/// Exchanges the ptlogin redirect for a real y.qq.com music credential.
///
/// This tail cannot go through
/// [`HttpClient`]: it needs per-request redirect suppression plus access to the
/// `Location` header, and the trait exposes neither (and reqwest follows
/// redirects by default, which would swallow the intermediate `Set-Cookie`
/// headers that carry `p_skey`). It therefore drives its own reqwest client with
/// a hand-rolled cookie jar. Every host in the chain is under `qq.com`, so a
/// flat jar is equivalent to a domain-scoped one here.
async fn qq_finish_music_login(
    jar: &mut BTreeMap<String, String>,
    redirect_url: &str,
) -> Result<String, String> {
    let agent = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| format!("QQ 登录客户端创建失败: {error}"))?;

    qq_follow_redirects(&agent, jar, redirect_url, QQ_PTLOGIN_REFERER).await?;
    let p_skey = jar.get("p_skey").cloned().unwrap_or_default();
    if p_skey.is_empty() {
        return Err("QQ 登录失败: 缺少 p_skey".to_owned());
    }

    let redirect_uri = format!(
        "https://y.qq.com/portal/wx_redirect.html?login_type=1&surl={}",
        query_escape("https://y.qq.com/")
    );
    let form = encode_query(&[
        ("response_type", "code".to_owned()),
        ("client_id", QQ_OAUTH_CLIENT_ID.to_owned()),
        ("redirect_uri", redirect_uri),
        ("scope", QQ_OAUTH_SCOPE.to_owned()),
        ("state", "state".to_owned()),
        ("switch", String::new()),
        ("from_ptlogin", "1".to_owned()),
        ("src", "1".to_owned()),
        ("update_auth", "1".to_owned()),
        ("openapi", "1010_1030".to_owned()),
        ("g_tk", gtk33(&p_skey).to_string()),
        ("auth_time", now_millis().to_string()),
        ("ui", qq_uuid()),
    ]);
    let response = agent
        .post(QQ_OAUTH_AUTHORIZE)
        .header(reqwest::header::USER_AGENT, UA_COMMON)
        .header(reqwest::header::ACCEPT, "*/*")
        .header(reqwest::header::REFERER, QQ_OAUTH_LOGIN_JUMP)
        .header(reqwest::header::ORIGIN, "https://graph.qq.com")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(reqwest::header::COOKIE, join_cookie_jar(jar))
        .body(form)
        .send()
        .await
        .map_err(|error| format!("QQ 授权请求失败: {error}"))?;
    let status = response.status().as_u16();
    absorb_set_cookies(jar, &raw_set_cookies(response.headers()));
    let location = header_value(response.headers(), reqwest::header::LOCATION);
    if !(200..400).contains(&status) {
        return Err(format!("QQ 授权请求失败: HTTP {status}"));
    }
    let final_url = if location.is_empty() {
        response.url().to_string()
    } else {
        qq_follow_redirects(
            &agent,
            jar,
            &resolve_url(QQ_OAUTH_AUTHORIZE, &location),
            QQ_OAUTH_AUTHORIZE,
        )
        .await?
    };
    let code = query_value(&final_url, "code");
    if code.is_empty() {
        return Err("QQ 授权失败: 未返回 code".to_owned());
    }

    let payload = serde_json::json!({
        "comm": { "g_tk": 5381, "platform": "yqq", "ct": 24, "cv": 0 },
        "req": {
            "module": "QQConnectLogin.LoginServer",
            "method": "QQLogin",
            "param": { "code": code },
        },
    });
    let response = agent
        .post(QQ_MUSIC_LOGIN_API)
        .header(reqwest::header::USER_AGENT, UA_COMMON)
        .header(reqwest::header::ACCEPT, "*/*")
        .header(reqwest::header::REFERER, "https://y.qq.com/")
        .header(reqwest::header::COOKIE, join_cookie_jar(jar))
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("QQ 音乐登录失败: {error}"))?;
    let status = response.status().as_u16();
    absorb_set_cookies(jar, &raw_set_cookies(response.headers()));
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("QQ 音乐登录响应读取失败: {error}"))?;
    if status != 200 {
        return Err(format!("QQ 音乐登录失败: HTTP {status}"));
    }
    let json = parse_json(&body, "QQ 音乐登录")?;
    let outer = json
        .get("code")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1);
    let req = json.get("req");
    let inner = req
        .and_then(|req| req.get("code"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1);
    if outer != 0 || inner != 0 {
        return Err(format!("QQ 音乐登录失败: code={outer} reqCode={inner}"));
    }
    for (name, value) in qq_login_data_cookies(req.and_then(|req| req.get("data"))) {
        jar.entry(name).or_insert(value);
    }
    normalize_qq_cookies(jar);
    if jar
        .get("qm_keyst")
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        return Err("QQ 音乐登录失败: 未获取到 qm_keyst".to_owned());
    }
    Ok(join_cookie_jar(jar))
}

/// Walks a redirect chain manually so each hop's `Set-Cookie` lands in `jar`.
/// Returns the last URL visited.
async fn qq_follow_redirects(
    agent: &reqwest::Client,
    jar: &mut BTreeMap<String, String>,
    start: &str,
    referer: &str,
) -> Result<String, String> {
    let mut current = start.trim().to_owned();
    let mut referer = referer.to_owned();
    for _ in 0..10 {
        if current.is_empty() {
            return Err("QQ 登录跳转地址为空".to_owned());
        }
        let response = agent
            .get(&current)
            .header(reqwest::header::USER_AGENT, UA_COMMON)
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::REFERER, &referer)
            .header(reqwest::header::COOKIE, join_cookie_jar(jar))
            .send()
            .await
            .map_err(|error| format!("QQ 登录跳转失败: {error}"))?;
        let status = response.status().as_u16();
        absorb_set_cookies(jar, &raw_set_cookies(response.headers()));
        let location = header_value(response.headers(), reqwest::header::LOCATION);
        if location.is_empty() || !(300..400).contains(&status) {
            return Ok(current);
        }
        referer = current.clone();
        current = resolve_url(&current, &location);
    }
    Err("QQ 登录跳转次数过多".to_owned())
}

fn raw_set_cookies(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    headers
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect()
}

fn header_value(headers: &reqwest::header::HeaderMap, name: reqwest::header::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Resolves a `Location` value against the URL it came from.
fn resolve_url(base: &str, reference: &str) -> String {
    let reference = reference.trim();
    if reference.contains("://") {
        return reference.to_owned();
    }
    let origin_end = base
        .find("://")
        .and_then(|scheme| base[scheme + 3..].find('/').map(|slash| scheme + 3 + slash))
        .unwrap_or(base.len());
    let (origin, path) = base.split_at(origin_end);
    if let Some(rest) = reference.strip_prefix("//") {
        let scheme = base.split_once("://").map_or("https", |(scheme, _)| scheme);
        return format!("{scheme}://{rest}");
    }
    if reference.starts_with('/') {
        return format!("{origin}{reference}");
    }
    let parent = path.rfind('/').map_or("/", |slash| &path[..=slash]);
    format!("{origin}{parent}{reference}")
}

/// A v4 UUID for the OAuth `ui` parameter.
fn qq_uuid() -> String {
    use rand::Rng;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill(&mut buf);
    buf[6] = (buf[6] & 0x0f) | 0x40;
    buf[8] = (buf[8] & 0x3f) | 0x80;
    let hex = hex::encode(buf);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Lifts the music credential out of the `QQLogin` response body.
///
/// The JSON carries the key under several
/// possible names and as either a string or a number.
fn qq_login_data_cookies(data: Option<&serde_json::Value>) -> Vec<(String, String)> {
    let Some(data) = data else {
        return Vec::new();
    };
    let value = |keys: &[&str]| -> String {
        for key in keys {
            match data.get(*key) {
                Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
                    return text.trim().to_owned()
                }
                Some(serde_json::Value::Number(number)) => {
                    let number = number.as_f64().unwrap_or_default();
                    if number > 0.0 {
                        return format!("{}", number as i64);
                    }
                }
                _ => {}
            }
        }
        String::new()
    };
    let mut cookies = Vec::with_capacity(6);
    let music_id = value(&["musicid", "musicId", "userid", "user_id", "uin"]);
    if !music_id.is_empty() {
        cookies.push(("musicid".to_owned(), music_id));
    }
    let music_key = value(&[
        "musickey",
        "music_key",
        "qqmusic_key",
        "qm_keyst",
        "strMusicKey",
    ]);
    if !music_key.is_empty() {
        cookies.push(("musickey".to_owned(), music_key.clone()));
        cookies.push(("qqmusic_key".to_owned(), music_key.clone()));
        cookies.push(("qm_keyst".to_owned(), music_key));
    }
    let refresh_key = value(&["refresh_key", "refreshKey"]);
    if !refresh_key.is_empty() {
        cookies.push(("refresh_key".to_owned(), refresh_key));
    }
    let refresh_token = value(&["refresh_token", "refreshToken"]);
    if !refresh_token.is_empty() {
        cookies.push(("refresh_token".to_owned(), refresh_token));
    }
    cookies
}

/// Fills in the aliases the qq source adapter reads (`uin`, `qqmusic_key`,
/// `qm_keyst`) from whichever equivalent the login actually produced.
fn normalize_qq_cookies(jar: &mut BTreeMap<String, String>) {
    let pick = |jar: &BTreeMap<String, String>, keys: &[&str]| -> String {
        for key in keys {
            if let Some(value) = jar.get(*key) {
                if !value.trim().is_empty() {
                    return value.trim().to_owned();
                }
            }
        }
        String::new()
    };
    for (target, sources) in [
        (
            "uin",
            &[
                "ptui_loginuin",
                "luin",
                "pt2gguin",
                "superuin",
                "p_uin",
                "musicid",
                "userid",
            ][..],
        ),
        (
            "qqmusic_key",
            &["qm_keyst", "musickey", "music_key", "p_skey", "skey"][..],
        ),
        (
            "qm_keyst",
            &["qqmusic_key", "musickey", "music_key", "p_skey", "skey"][..],
        ),
    ] {
        if pick(&*jar, &[target]).is_empty() {
            let value = pick(&*jar, sources);
            if !value.is_empty() {
                jar.insert(target.to_owned(), value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netease_status_codes_map_to_domain_states() {
        assert!(matches!(netease_status(801), QrStatus::Pending));
        assert!(matches!(netease_status(802), QrStatus::Scanned));
        assert!(matches!(netease_status(803), QrStatus::Confirmed));
        assert!(matches!(netease_status(800), QrStatus::Expired));
        assert!(matches!(netease_status(400), QrStatus::Failed));
    }

    #[test]
    fn bilibili_status_codes_map_to_domain_states() {
        assert!(matches!(bilibili_status(86101), QrStatus::Pending));
        assert!(matches!(bilibili_status(86090), QrStatus::Scanned));
        assert!(matches!(bilibili_status(0), QrStatus::Confirmed));
        assert!(matches!(bilibili_status(86038), QrStatus::Expired));
        assert!(matches!(bilibili_status(-1), QrStatus::Failed));
    }

    #[test]
    fn qq_status_codes_are_strings_not_numbers() {
        assert!(matches!(qq_status("66"), QrStatus::Pending));
        assert!(matches!(qq_status("67"), QrStatus::Scanned));
        assert!(matches!(qq_status("0"), QrStatus::Confirmed));
        assert!(matches!(qq_status("65"), QrStatus::Expired));
        assert!(matches!(qq_status("68"), QrStatus::Failed));
        assert!(matches!(qq_status(""), QrStatus::Failed));
    }

    #[test]
    fn set_cookie_join_keeps_crumbs_sorted_and_drops_blanks() {
        let raw = vec![
            "SESSDATA=abc; Path=/; HttpOnly; Domain=.bilibili.com".to_owned(),
            "bili_jct=def; Path=/".to_owned(),
            "=orphan; Path=/".to_owned(),
            "no-equals-sign".to_owned(),
            "DedeUserID=42".to_owned(),
        ];
        assert_eq!(
            join_set_cookies(&raw),
            "DedeUserID=42; SESSDATA=abc; bili_jct=def"
        );
    }

    #[test]
    fn set_cookie_join_lets_later_values_win() {
        let raw = vec!["a=1; Path=/".to_owned(), "a=2; Path=/".to_owned()];
        assert_eq!(join_set_cookies(&raw), "a=2");
    }

    #[test]
    fn ptui_callback_yields_code_redirect_and_message() {
        let raw = "ptuiCB('0','0','https://ptlogin2.qq.com/check_sig?uin=123','0','\
登录成功！', 'nick');";
        let (code, message, redirect) = parse_qq_check(raw);
        assert_eq!(code, "0");
        assert_eq!(redirect, "https://ptlogin2.qq.com/check_sig?uin=123");
        assert_eq!(message, "登录成功！");
    }

    #[test]
    fn ptui_callback_waiting_state_parses() {
        let raw = "ptuiCB('66','0','','0','二维码未失效。', '');";
        let (code, message, redirect) = parse_qq_check(raw);
        assert_eq!(code, "66");
        assert!(redirect.is_empty());
        assert_eq!(message, "二维码未失效。");
        assert!(matches!(qq_status(&code), QrStatus::Pending));
    }

    #[test]
    fn non_callback_body_is_surfaced_verbatim() {
        let (code, message, redirect) = parse_qq_check("<html>502</html>");
        assert!(code.is_empty());
        assert!(redirect.is_empty());
        assert_eq!(message, "<html>502</html>");
        assert!(matches!(qq_status(&code), QrStatus::Failed));
    }

    #[test]
    fn hash33_matches_known_values() {
        // Derived by hand: h = h*33 + c, masked to 31 bits.
        // "a" -> 97, "ab" -> 97*33 + 98 = 3299, "abc" -> 3299*33 + 99 = 108966.
        assert_eq!(hash33(""), 0);
        assert_eq!(hash33("a"), 97);
        assert_eq!(hash33("ab"), 3299);
        assert_eq!(hash33("abc"), 108_966);
        // 30 chars: long enough that the i64 accumulator must wrap before masking.
        assert_eq!(hash33("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"), 2_012_202_494);
    }

    #[test]
    fn gtk33_seeds_with_5381() {
        assert_eq!(gtk33(""), 5381);
        assert_eq!(gtk33("a"), 5381 * 33 + 97);
    }

    #[test]
    fn qq_state_round_trips_through_the_opaque_key() {
        let mut jar = BTreeMap::new();
        jar.insert("qrsig".to_owned(), "sig+value/with=pad".to_owned());
        jar.insert("pt_login_sig".to_owned(), "login".to_owned());
        let key = encode_query(&[("session", encode_qq_state(&jar))]);
        assert_eq!(decode_qq_state(&query_value(&key, "session")), jar);
    }

    #[test]
    fn location_header_resolution_handles_relative_forms() {
        let base = "https://graph.qq.com/oauth2.0/authorize";
        assert_eq!(
            resolve_url(base, "https://y.qq.com/?code=1"),
            "https://y.qq.com/?code=1"
        );
        assert_eq!(
            resolve_url(base, "/login_jump"),
            "https://graph.qq.com/login_jump"
        );
        assert_eq!(
            resolve_url(base, "show?code=2"),
            "https://graph.qq.com/oauth2.0/show?code=2"
        );
        assert_eq!(resolve_url(base, "//y.qq.com/x"), "https://y.qq.com/x");
    }

    #[test]
    fn query_value_reads_from_full_urls() {
        assert_eq!(
            query_value("https://y.qq.com/portal?login_type=1&code=ABC#frag", "code"),
            "ABC"
        );
        assert_eq!(query_value("https://y.qq.com/portal", "code"), "");
    }

    #[test]
    fn normalize_fills_music_key_aliases() {
        let mut jar = BTreeMap::new();
        jar.insert("musickey".to_owned(), "KEY".to_owned());
        jar.insert("p_uin".to_owned(), "o123".to_owned());
        normalize_qq_cookies(&mut jar);
        assert_eq!(jar.get("qm_keyst").map(String::as_str), Some("KEY"));
        assert_eq!(jar.get("qqmusic_key").map(String::as_str), Some("KEY"));
        assert_eq!(jar.get("uin").map(String::as_str), Some("o123"));
    }

    #[test]
    fn rendered_qr_is_a_decodable_png_of_the_expected_size() {
        let url = "https://music.163.com/login?codekey=abcdef0123456789";
        let data_url = qr_data_url(url).expect("qr render");
        let encoded = data_url
            .strip_prefix("data:image/png;base64,")
            .expect("data url prefix");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64 payload");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]));
        // IHDR width/height live at byte 16..24 and must cover modules + quiet zone.
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        assert_eq!(width, height);
        let modules = qrcode::QrCode::new(url.as_bytes())
            .expect("qr encode")
            .width() as u32;
        assert_eq!(width, (modules + 8) * 6);
        // IEND terminates the stream: 4-byte length, type, empty body, CRC.
        assert!(bytes.ends_with(&[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[tokio::test]
    async fn unsupported_sources_are_rejected_before_any_request() {
        let client = crate::infrastructure::http::FakeHttpClient {
            response: HttpResponse::default(),
        };
        assert!(create(&client, "kugou").await.is_err());
        assert!(check(&client, "kugou", "key").await.is_err());
        assert!(check(&client, "netease", "  ").await.is_err());
    }
}
