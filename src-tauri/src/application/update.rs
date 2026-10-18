//! Update check against the GitHub releases API.
//!
//! Fetches `releases/latest`, normalizes both versions the same way, and
//! compares them numerically so `v1.10.0` beats `v1.9.0` (a lexical compare
//! gets that backwards). The optional proxy prefix serves networks that cannot
//! reach github.com directly.

use serde::{Deserialize, Serialize};

use crate::infrastructure::http::{HttpClient, HttpError};

/// This app's own repository, as `owner/repo`.
pub const DEFAULT_REPO: &str = "ouraihub/AutoOwl";

/// Proxy used when the direct API call is blocked.
pub const DEFAULT_PROXY: &str = "https://edgeone.gh-proxy.com";

const API_BASE: &str = "https://api.github.com";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub available: bool,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    html_url: String,
}

/// The build's own version, with no network call.
///
/// Used as the answer when the check is disabled or fails: the settings page
/// still needs a current version to render.
pub fn current() -> UpdateInfo {
    UpdateInfo {
        current_version: normalize_version(env!("CARGO_PKG_VERSION")),
        latest_version: None,
        release_url: None,
        available: false,
    }
}

/// Queries the latest release and reports whether it is newer than this build.
///
/// GitHub rejects API requests without a `User-Agent`, so one is always sent.
pub async fn check<C: HttpClient>(
    client: &C,
    repo: &str,
    proxy: Option<&str>,
) -> Result<UpdateInfo, HttpError> {
    let repo = repo.trim().trim_matches('/');
    let repo = if repo.is_empty() { DEFAULT_REPO } else { repo };
    let direct = format!("{API_BASE}/repos/{repo}/releases/latest");
    let url = match proxy.map(str::trim).filter(|value| !value.is_empty()) {
        Some(prefix) => format!("{}/{direct}", prefix.trim_end_matches('/')),
        None => direct,
    };

    let headers = [
        (
            "User-Agent",
            format!("autoowl/{}", env!("CARGO_PKG_VERSION")),
        ),
        ("Accept", "application/vnd.github+json".to_owned()),
    ];
    let response = client.get_with_headers(&url, &headers).await?;
    if !(200..300).contains(&response.status) {
        return Err(HttpError::Failed(format!(
            "GitHub 版本查询返回 HTTP {}",
            response.status
        )));
    }
    let release: GithubRelease = serde_json::from_slice(&response.body)
        .map_err(|error| HttpError::Failed(format!("GitHub 版本响应解析失败: {error}")))?;

    let mut latest = normalize_version(&release.tag_name);
    if latest.is_empty() {
        latest = normalize_version(&release.name);
    }
    let current_version = normalize_version(env!("CARGO_PKG_VERSION"));
    let available = !latest.is_empty() && compare_versions(&latest, &current_version).is_gt();
    let release_url = {
        let trimmed = release.html_url.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    };

    Ok(UpdateInfo {
        current_version,
        latest_version: (!latest.is_empty()).then_some(latest),
        release_url,
        available,
    })
}

/// Strips `refs/tags/` and a leading `v`, then keeps the leading run of digits,
/// dots, dashes and underscores.
///
/// It stops at the first other character, so `1.2.3-beta+build` becomes
/// `1.2.3-beta`.
fn normalize_version(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_prefix("refs/tags/").unwrap_or(raw);
    let raw = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .unwrap_or(raw);
    let kept: String = raw
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
        .collect();
    kept.trim_matches(|c| matches!(c, '.' | '-' | '_'))
        .to_owned()
}

/// Numeric, component-wise comparison. Missing components count as zero, so
/// `1.2` and `1.2.0` are equal.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let left = version_parts(a);
    let right = version_parts(b);
    let width = left.len().max(right.len());
    for index in 0..width {
        let lhs = left.get(index).copied().unwrap_or(0);
        let rhs = right.get(index).copied().unwrap_or(0);
        match lhs.cmp(&rhs) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Splits a normalized version into numbers; a non-numeric component counts as
/// zero.
fn version_parts(version: &str) -> Vec<u64> {
    let normalized = normalize_version(version);
    if normalized.is_empty() {
        return vec![0];
    }
    let parts: Vec<u64> = normalized
        .split(['.', '-', '_'])
        .filter(|field| !field.is_empty())
        .map(|field| field.parse().unwrap_or(0))
        .collect();
    if parts.is_empty() {
        vec![0]
    } else {
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_versions, normalize_version};
    use std::cmp::Ordering;

    #[test]
    fn normalizes_the_tag_shapes_github_actually_returns() {
        assert_eq!(normalize_version("v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("refs/tags/v1.2.3"), "1.2.3");
        assert_eq!(normalize_version("  V2.0  "), "2.0");
        // Stops at the first character outside the allowed set, so build
        // metadata is dropped rather than making the version uncomparable.
        assert_eq!(normalize_version("1.2.3+build7"), "1.2.3");
        assert_eq!(normalize_version("nightly"), "");
    }

    #[test]
    fn compares_components_numerically_not_lexically() {
        // The regression this defends: a string compare puts "1.10.0" before
        // "1.9.0" and would hide a real update.
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.9.0", "1.10.0"), Ordering::Less);
    }

    #[test]
    fn treats_missing_trailing_components_as_zero() {
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.2.1", "1.2"), Ordering::Greater);
    }

    #[test]
    fn an_unparseable_version_ranks_below_any_release() {
        assert_eq!(compare_versions("", "0.1.0"), Ordering::Less);
        assert_eq!(compare_versions("nightly", "0.0.1"), Ordering::Less);
    }
}
