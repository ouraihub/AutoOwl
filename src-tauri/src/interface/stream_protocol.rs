//! `stream://` playback proxy, the equivalent of go-music-dl's `/download?stream=1`.
//!
//! `<audio>` cannot set Referer/User-Agent/Cookie, and music CDNs check them:
//! bilibili mirrors 403 without the bilibili Referer, migu needs a mobile UA.
//! So the WebView plays `http://stream.localhost/?source=<id>&url=<cdn url>` and
//! this handler fetches the upstream with `source_headers`, passing Range through.

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::infrastructure::http::source_headers;
use crate::state::AppState;

/// Largest slice served per request. The protocol API returns whole bodies, so
/// an open `bytes=0-` is answered in slices and the element asks for the rest.
const CHUNK: u64 = 2 * 1024 * 1024;

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let response = match proxy(&app, &request).await {
            Ok(response) => response,
            Err((status, message)) => {
                tracing::warn!(event = "stream_proxy_failed", %status, %message);
                plain(status, message)
            }
        };
        responder.respond(response);
    });
}

async fn proxy<R: Runtime>(
    app: &tauri::AppHandle<R>,
    request: &Request<Vec<u8>>,
) -> Result<Response<Vec<u8>>, (StatusCode, String)> {
    let query = tauri::Url::parse(&format!("http://stream.localhost{}", request.uri()))
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let param = |name: &str| {
        query
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let (source, url) = param("source").zip(param("url")).ok_or((
        StatusCode::BAD_REQUEST,
        "source and url required".to_owned(),
    ))?;
    // Only remote http(s): the handler must not become a local file reader.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err((StatusCode::BAD_REQUEST, "url must be http(s)".to_owned()));
    }

    let (start, end) = range(request.headers().get(header::RANGE));
    let state = app.state::<AppState>();
    let mut upstream = state.http.raw().get(&url).header(
        header::RANGE,
        format!("bytes={start}-{}", end.min(start + CHUNK - 1)),
    );
    for (name, value) in source_headers(&source, &state.cookies.get(&source)) {
        upstream = upstream.header(name, value);
    }
    let reply = upstream
        .send()
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?;

    let mut response = Response::builder()
        .status(reply.status().as_u16())
        .header(header::ACCEPT_RANGES, "bytes");
    for name in [header::CONTENT_TYPE, header::CONTENT_RANGE] {
        if let Some(value) = reply.headers().get(&name) {
            response = response.header(name, value.as_bytes());
        }
    }
    let body = reply
        .bytes()
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?;
    response
        .body(body.to_vec())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

/// `bytes=S-` / `bytes=S-E` → (S, E); absent or unparsable means from 0, open.
fn range(value: Option<&header::HeaderValue>) -> (u64, u64) {
    let spec = value
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .unwrap_or("0-");
    let (start, end) = spec.split_once('-').unwrap_or(("0", ""));
    (
        start.trim().parse().unwrap_or(0),
        end.trim().parse().unwrap_or(u64::MAX),
    )
}

fn plain(status: StatusCode, message: String) -> Response<Vec<u8>> {
    let mut response = Response::new(message.into_bytes());
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use super::range;
    use tauri::http::HeaderValue;

    #[test]
    fn range_header_forms() {
        assert_eq!(range(None), (0, u64::MAX));
        assert_eq!(
            range(Some(&HeaderValue::from_static("bytes=0-"))),
            (0, u64::MAX)
        );
        assert_eq!(
            range(Some(&HeaderValue::from_static("bytes=100-199"))),
            (100, 199)
        );
        assert_eq!(
            range(Some(&HeaderValue::from_static("bytes=junk"))),
            (0, u64::MAX)
        );
    }
}
