//! `autoowl-cli`: drives the running autoowl app over its local MCP server, so
//! scripts and AI agents that prefer a shell can search and play songs.
//!
//! Needs the app running with "AI 接入" started and a token from that page:
//!   set AUTOOWL_TOKEN=autoowl_...      (optional: AUTOOWL_URL, default below)
//!   autoowl-cli search 千千阙歌
//!   autoowl-cli play 0
//!   autoowl-cli pause | resume | next | previous | status

use serde_json::{json, Value};

const DEFAULT_URL: &str = "http://127.0.0.1:37661/mcp";
const USAGE: &str =
    "usage: autoowl-cli <search <query>|play <index>|pause|resume|next|previous|status>";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let code = match run().await {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(error) => {
            eprintln!("autoowl-cli: {error}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<String, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (tool, arguments) = parse(&args)?;
    let token = std::env::var("AUTOOWL_TOKEN")
        .map_err(|_| "set AUTOOWL_TOKEN (create one on the app's \"AI 接入\" page)".to_owned())?;
    let url = std::env::var("AUTOOWL_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());

    let reply: Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .bearer_auth(token)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        }))
        .send()
        .await
        .map_err(|e| format!("cannot reach {url} (is the app running with MCP started?): {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(error) = reply.get("error") {
        return Err(error["message"]
            .as_str()
            .unwrap_or("request failed")
            .to_owned());
    }
    let result = &reply["result"];
    if result["isError"].as_bool() == Some(true) {
        return Err(result["content"][0]["text"]
            .as_str()
            .unwrap_or("tool failed")
            .to_owned());
    }
    serde_json::to_string_pretty(&result["structuredContent"]).map_err(|e| e.to_string())
}

/// Maps argv to an MCP tool name and its arguments.
fn parse(args: &[String]) -> Result<(&'static str, Value), String> {
    let rest = args.get(1..).unwrap_or_default().join(" ");
    match args.first().map(String::as_str) {
        Some("search") if !rest.trim().is_empty() => Ok(("search_songs", json!({ "query": rest }))),
        Some("play") => rest
            .trim()
            .parse::<u64>()
            .map(|index| ("play_song", json!({ "index": index })))
            .map_err(|_| USAGE.to_owned()),
        Some("status") => Ok(("player_status", json!({}))),
        Some("pause") => Ok(("pause", json!({}))),
        Some("resume") => Ok(("resume", json!({}))),
        Some("next") => Ok(("next", json!({}))),
        Some("previous") => Ok(("previous", json!({}))),
        _ => Err(USAGE.to_owned()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::parse;
    use serde_json::json;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn maps_commands_to_tools() {
        assert_eq!(
            parse(&argv("search 千千阙歌 陈慧娴")).unwrap(),
            ("search_songs", json!({ "query": "千千阙歌 陈慧娴" }))
        );
        assert_eq!(
            parse(&argv("play 3")).unwrap(),
            ("play_song", json!({ "index": 3 }))
        );
        assert_eq!(parse(&argv("status")).unwrap().0, "player_status");
        assert_eq!(parse(&argv("next")).unwrap().0, "next");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&argv("search")).is_err());
        assert!(parse(&argv("play abc")).is_err());
        assert!(parse(&argv("")).is_err());
        assert!(parse(&argv("dance")).is_err());
    }
}
