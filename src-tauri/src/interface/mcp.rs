//! Music tools AI can call, offered on two doors that share one catalogue and one
//! [`run`]: Aether's loopback MCP server (Claude Desktop, Cursor, `autoowl-cli`)
//! and the in-app chat ([`AutoOwlHost`]).
//!
//! Search runs here in Rust. Playback cannot: the stream URL is resolved and fed
//! to the WebView's `<audio>` element by the frontend player store, so control
//! tools emit `mcp:player` and `PlayerBar` performs the action exactly as if the
//! user had clicked. One code path for UI and AI, no second player.

use std::sync::{Arc, Mutex, OnceLock};

use aether_mcp_server::{
    CallContext, ExposedTool, KitError, KitErrorCode, ScopeSpec, ToolHandler,
    DEFAULT_MAX_REQUEST_BYTES,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_aether::{
    Approval, CancellationToken, HostTool, KitHost, ModelRef, RunRequest, ToolCall, ToolError,
    ToolOutput, ToolSource, ToolSpec,
};

use crate::application::search::{aggregate_search, SOURCE_TIMEOUT};
use crate::domain::source::{SearchQuery, Song};
use crate::state::AppState;

pub const SCOPE_READ: &str = "music.read";
pub const SCOPE_CONTROL: &str = "music.control";

/// Set in Tauri `setup`; the MCP server is built before the app exists.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Songs from the last `search_songs` call, so `play_song` can take an index.
// ponytail: one shared list for all MCP clients; key by token id if two AIs ever drive one app.
static LAST_RESULTS: Mutex<Vec<Song>> = Mutex::new(Vec::new());

const MAX_RESULTS: usize = 10;

pub fn bind_app(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn scopes() -> Vec<ScopeSpec> {
    vec![
        ScopeSpec {
            id: SCOPE_READ,
            label_key: "music.scope.read",
            destructive: false,
        },
        ScopeSpec {
            id: SCOPE_CONTROL,
            label_key: "music.scope.control",
            destructive: false,
        },
    ]
}

/// One music tool, as both doors describe it.
struct Spec {
    name: &'static str,
    description: &'static str,
    /// Needs arguments; the rest take `{}`.
    params: Option<fn() -> Value>,
    scope: &'static str,
    read_only: bool,
}

const CATALOGUE: [Spec; 7] = [
    Spec {
        name: "search_songs",
        description: "Search songs across all music platforms. Returns up to 10 results with an `index`; pass that index to `play_song`.",
        params: Some(|| {
            json!({
                "type": "object",
                "properties": { "query": { "type": "string", "description": "Song title and/or artist, e.g. \"千千阙歌 陈慧娴\"" } },
                "required": ["query"]
            })
        }),
        scope: SCOPE_READ,
        read_only: true,
    },
    Spec {
        name: "play_song",
        description: "Play a song from the last `search_songs` result by its `index`.",
        params: Some(|| {
            json!({
                "type": "object",
                "properties": { "index": { "type": "integer", "minimum": 0 } },
                "required": ["index"]
            })
        }),
        scope: SCOPE_CONTROL,
        read_only: false,
    },
    Spec {
        name: "player_status",
        description: "Current track, play state and queue length.",
        params: None,
        scope: SCOPE_READ,
        read_only: true,
    },
    Spec {
        name: "pause",
        description: "Pause playback.",
        params: None,
        scope: SCOPE_CONTROL,
        read_only: false,
    },
    Spec {
        name: "resume",
        description: "Resume playback of the current track.",
        params: None,
        scope: SCOPE_CONTROL,
        read_only: false,
    },
    Spec {
        name: "next",
        description: "Skip to the next track in the queue.",
        params: None,
        scope: SCOPE_CONTROL,
        read_only: false,
    },
    Spec {
        name: "previous",
        description: "Go back to the previous track in the queue.",
        params: None,
        scope: SCOPE_CONTROL,
        read_only: false,
    },
];

impl Spec {
    fn schema(&self) -> Value {
        self.params
            .map_or_else(|| json!({ "type": "object", "properties": {} }), |f| f())
    }
}

/// Runs one catalogue tool; the single implementation behind both doors.
async fn run(name: &str, args: Value) -> Result<Value, KitError> {
    match name {
        "search_songs" => search(args).await,
        "play_song" => play(args),
        "player_status" => status(),
        "pause" | "resume" | "next" | "previous" => control(name),
        _ => Err(KitError::not_found("tool")),
    }
}

/// The MCP door.
pub fn tools() -> Vec<ExposedTool> {
    CATALOGUE
        .iter()
        .map(|spec| {
            let name = spec.name;
            let handler: ToolHandler = Arc::new(move |args: Value, _ctx: CallContext| {
                Box::pin(async move { run(name, args).await })
            });
            ExposedTool {
                name,
                description: spec.description,
                input_schema: spec.schema(),
                scope: spec.scope,
                read_only: spec.read_only,
                destructive: false,
                idempotent: spec.read_only,
                max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
                handler,
            }
        })
        .collect()
}

const SYSTEM_PROMPT: &str = "你是 AutoOwl 的音乐助手，能搜索并播放歌曲。\
用户想听歌时，先调用 search_songs，再用结果里的 index 调用 play_song；\
没有明确指定时，选第一条和用户描述最匹配的结果。\
暂停、继续、切歌用对应工具。回复用中文，一两句话说明结果即可。";

/// `app_settings` key of the model the chat last ran with (JSON `ModelRef`).
const CHAT_MODEL_KEY: &str = "chat.model";

/// The in-app chat door: the same catalogue as [`tools`], run in-process.
///
/// Aether's `ChatPanel` starts with no model unless the host passes one, so the
/// user had to pick it on every visit. The host remembers the last model used and
/// hands it back through [`default_chat_model`], AutoNote-style but without a
/// separate roles page: using a model is what makes it the default.
pub struct AutoOwlHost {
    /// Separate handle on `providers.db` (the plugin owns another), as in AutoNote.
    pub providers: aether_providers::Store,
}

impl AutoOwlHost {
    /// Chat models that can call tools; a music assistant without tools can't play anything.
    fn chat_models(&self) -> Result<Vec<ModelRef>, KitError> {
        Ok(self
            .providers
            .model_options(aether_providers::ModelFilter {
                model_type: Some(aether_providers::ModelType::Chat),
                supports_tools: None,
            })?
            .into_iter()
            // `supports_tools: None` means unknown; only drop models known not to.
            .filter(|option| option.supports_tools != Some(false))
            .map(|option| ModelRef {
                endpoint_id: option.endpoint_id,
                model_id: option.model_id,
            })
            .collect())
    }

    /// The remembered model if it still exists, otherwise the first usable one.
    pub fn default_chat_model(&self) -> Result<Option<ModelRef>, KitError> {
        let models = self.chat_models()?;
        let remembered = remembered_model();
        Ok(pick(&models, None, remembered.as_ref()))
    }
}

/// Request model, then remembered model, then the first available, each only if
/// it is still among `models` (a deleted or disabled model is skipped).
fn pick(
    models: &[ModelRef],
    requested: Option<&ModelRef>,
    remembered: Option<&ModelRef>,
) -> Option<ModelRef> {
    [requested, remembered]
        .into_iter()
        .flatten()
        .find(|model| models.contains(model))
        .or_else(|| models.first())
        .cloned()
}

fn remembered_model() -> Option<ModelRef> {
    let state = APP.get()?.state::<AppState>();
    let json: String = state
        .database
        .with(|c| {
            c.query_row(
                "SELECT value_json FROM app_settings WHERE key = ?1",
                [CHAT_MODEL_KEY],
                |row| row.get(0),
            )
        })
        .ok()?;
    serde_json::from_str(&json).ok()
}

fn remember_model(model: &ModelRef) {
    let Some(app) = APP.get() else { return };
    let Ok(json) = serde_json::to_string(model) else {
        return;
    };
    let result = app.state::<AppState>().database.with(|c| {
        c.execute(
            "INSERT INTO app_settings(key, value_json, updated_at) VALUES(?1, ?2, ?3) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            rusqlite::params![CHAT_MODEL_KEY, json, crate::domain::clock::now_iso8601()],
        )
    });
    if let Err(error) = result {
        tracing::warn!(event = "chat_model_remember_failed", %error);
    }
}

/// Model `ChatPanel` should start with, so the user isn't asked to pick one each visit.
/// `null` when no usable chat model is configured; the panel then guides to settings.
#[tauri::command]
pub fn chat_default_model(
    host: tauri::State<'_, Arc<AutoOwlHost>>,
) -> Result<Option<ModelRef>, crate::error::IpcError> {
    host.default_chat_model().map_err(|error| {
        crate::error::IpcError::new(
            "STATE_UNAVAILABLE",
            error.message(),
            true,
            "chat".to_owned(),
        )
    })
}

#[async_trait::async_trait]
impl KitHost for AutoOwlHost {
    fn select_model(&self, req: &RunRequest) -> Result<ModelRef, KitError> {
        let models = self.chat_models()?;
        let remembered = remembered_model();
        let chosen = pick(&models, req.model.as_ref(), remembered.as_ref())
            .ok_or_else(|| KitError::needs_config("model"))?;
        if remembered.as_ref() != Some(&chosen) {
            remember_model(&chosen);
        }
        Ok(chosen)
    }

    fn system_prompt(&self, _req: &RunRequest) -> Option<String> {
        Some(SYSTEM_PROMPT.to_owned())
    }

    fn tools(&self, _req: &RunRequest) -> Vec<HostTool> {
        CATALOGUE
            .iter()
            .map(|spec| HostTool {
                spec: ToolSpec {
                    name: spec.name.to_owned(),
                    description: spec.description.to_owned(),
                    parameters: spec.schema(),
                    // Every action is undoable in one click; asking each time would
                    // break "play me a song". Switch a tool to `Ask` to confirm it.
                    approval: Approval::Auto,
                    source: ToolSource::Host,
                },
            })
            .collect()
    }

    async fn call_tool(
        &self,
        call: &ToolCall,
        _cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if !CATALOGUE.iter().any(|spec| spec.name == call.name) {
            return Err(ToolError::not_found());
        }
        match run(&call.name, call.args.clone()).await {
            Ok(value) => Ok(ToolOutput {
                text: value.to_string(),
                payload: Some(value),
            }),
            Err(error) => Err(ToolError {
                // `KitErrorCode` serializes as its snake_case wire name.
                code: serde_json::to_value(error.code())
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                message: error.message().to_owned(),
                // Every failure in `run` happens before an event is emitted.
                unknown: false,
            }),
        }
    }
}

fn app() -> Result<&'static AppHandle, KitError> {
    APP.get().ok_or_else(|| KitError::conflict("app not ready"))
}

async fn search(args: Value) -> Result<Value, KitError> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| KitError::bad_request("query", "required"))?
        .to_owned();
    let state = app()?.state::<AppState>();
    let ids = state.sources.ids();
    let sources = ids.iter().filter_map(|id| state.sources.get(id)).collect();
    let result = aggregate_search(
        sources,
        SearchQuery {
            query,
            page: 1,
            page_size: MAX_RESULTS as u32,
            sources: ids,
        },
        SOURCE_TIMEOUT,
    )
    .await
    .map_err(|error| KitError::new(KitErrorCode::Upstream, error.message))?;

    let songs: Vec<Song> = result
        .items
        .into_iter()
        .take(MAX_RESULTS)
        .map(|ranked| ranked.song)
        .collect();
    let rows: Vec<Value> = songs
        .iter()
        .enumerate()
        .map(|(index, song)| {
            let mut row = describe(song);
            row["index"] = json!(index);
            row
        })
        .collect();
    *LAST_RESULTS
        .lock()
        .map_err(|_| KitError::conflict("results lock"))? = songs;
    Ok(json!({ "results": rows }))
}

fn play(args: Value) -> Result<Value, KitError> {
    let index = args
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| KitError::bad_request("index", "required"))? as usize;
    let song = LAST_RESULTS
        .lock()
        .map_err(|_| KitError::conflict("results lock"))?
        .get(index)
        .cloned()
        .ok_or_else(|| KitError::bad_request("index", "out_of_range_call_search_songs_first"))?;
    let summary = describe(&song);
    emit(json!({ "action": "play", "song": song }))?;
    Ok(json!({ "playing": summary }))
}

fn control(action: &str) -> Result<Value, KitError> {
    emit(json!({ "action": action }))?;
    Ok(json!({ "ok": true, "action": action }))
}

fn status() -> Result<Value, KitError> {
    let state = app()?.state::<AppState>();
    let player = state
        .player
        .lock()
        .map_err(|_| KitError::conflict("player lock"))?;
    Ok(json!({
        "status": player.status,
        "current": player.current.as_ref().map(describe),
        // No position: the frontend keeps it local on purpose (timeupdate fires ~4x/s).
        "durationSec": player.duration_ms / 1000,
        "queueLength": player.queue.len(),
    }))
}

fn emit(payload: Value) -> Result<(), KitError> {
    app()?
        .emit("mcp:player", payload)
        .map_err(KitError::internal)
}

fn describe(song: &Song) -> Value {
    let artists: Vec<&str> = song.artists.iter().map(|a| a.name.as_str()).collect();
    json!({
        "title": song.title,
        "artist": artists.join(" / "),
        "album": song.album.as_ref().map(|a| a.name.as_str()),
        "source": song.reference.source,
        "durationSec": song.duration_ms.map(|ms| ms / 1000),
    })
}

#[cfg(test)]
mod tests {
    use super::{pick, ModelRef};

    fn model(id: &str) -> ModelRef {
        ModelRef {
            endpoint_id: "ep".to_owned(),
            model_id: id.to_owned(),
        }
    }

    #[test]
    fn chat_model_precedence_skips_models_that_are_gone() {
        let models = [model("a"), model("b"), model("c")];
        let (a, b, c, gone) = (model("a"), model("b"), model("c"), model("gone"));
        // What the user picked wins.
        assert_eq!(pick(&models, Some(&c), Some(&b)), Some(c));
        // Nothing picked: the remembered model, not the first.
        assert_eq!(pick(&models, None, Some(&b)), Some(b.clone()));
        // A deleted pick or remembered model is skipped, never sent.
        assert_eq!(pick(&models, Some(&gone), Some(&b)), Some(b));
        assert_eq!(pick(&models, None, Some(&gone)), Some(a));
        // No usable model at all: the caller reports needs_config.
        assert_eq!(pick(&[], None, Some(&gone)), None);
    }
}
