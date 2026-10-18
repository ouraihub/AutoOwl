#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// `unwrap_used`/`expect_used` are denied crate-wide for production code
// (docs/工程规范与质量门禁.md §50-51). Tests are exempt: a panic there is the
// failure report, and `--all-targets` must stay clean under `-D warnings`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod application;
pub mod config;
pub mod domain;
pub mod error;
pub mod infrastructure;
pub mod interface;
mod logging;
pub mod state;

/// Default MCP port; the settings page can change it (persisted in `mcp-server.db`).
const MCP_PORT: u16 = 37661;

pub fn run() {
    // Before the builder, so plugin setup is logged too. A windowed release build
    // has no console, which is why logs must go to a file.
    let root = logging::data_root();
    let logs = match logging::init(&root) {
        Ok(logs) => logs,
        Err(error) => {
            eprintln!("autoowl: logging failed to start: {error}");
            return;
        }
    };
    tracing::info!(event = "app_start", path = %root.display(), "AutoOwl starting");

    // MCP server (external AI) and in-app chat share one tool catalogue
    // (interface::mcp). Model keys live in providers.db, history in chat.db.
    let host = std::sync::Arc::new(interface::mcp::AutoOwlHost {
        providers: match aether_providers::Store::open(&root) {
            Ok(store) => store,
            Err(error) => {
                tracing::error!(event = "aether_init_failed", %error);
                return;
            }
        },
    });
    let aether = (|| {
        let server = aether_mcp_server::McpServer::new(
            &root,
            aether_mcp_server::ServerConfig {
                default_port: MCP_PORT,
                token_prefix: "autoowl",
            },
            interface::mcp::scopes(),
            interface::mcp::tools(),
        )?;
        let history: std::sync::Arc<dyn tauri_plugin_aether::ConversationStore> =
            std::sync::Arc::new(aether_chat::SqliteStore::open(&root)?);
        tauri_plugin_aether::Builder::new(host.clone(), logs)
            .mcp_server(server)
            .providers(aether_providers::Store::open(&root)?)
            .chat(tauri_plugin_aether::ChatConfig::default(), Some(history))
            .build()
    })();
    let aether = match aether {
        Ok(plugin) => plugin,
        Err(error) => {
            tracing::error!(event = "aether_init_failed", %error);
            return;
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(aether)
        .setup(move |app| {
            use tauri::Manager;
            tracing::info!(event = "data_root_resolved", path = %root.display());
            app.manage(state::AppState::new(root.clone()));
            app.manage(host.clone());
            interface::mcp::bind_app(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            interface::commands::health,
            interface::commands::validate_search,
            interface::commands::validate_collection,
            interface::commands::normalize_lyrics_command,
            interface::commands::search_songs,
            interface::commands::search_collections,
            interface::commands::get_collection_detail,
            interface::commands::parse_share_link,
            interface::commands::get_lyrics,
            interface::commands::get_source_capabilities,
            interface::commands::resolve_playback,
            interface::commands::switch_source,
            interface::player_commands::player_load,
            interface::player_commands::player_play,
            interface::player_commands::player_pause,
            interface::player_commands::player_next,
            interface::player_commands::player_previous,
            interface::player_commands::player_seek,
            interface::player_commands::player_report_duration,
            interface::player_commands::player_set_mode,
            interface::player_commands::player_get_state,
            interface::player_commands::player_set_volume,
            interface::player_commands::player_set_queue,
            interface::player_commands::player_remove_from_queue,
            interface::player_commands::player_clear_queue,
            interface::player_commands::player_move_in_queue,
            interface::admin_commands::get_auth_status,
            interface::admin_commands::list_auth_status,
            interface::auth_commands::qr_login_create,
            interface::auth_commands::qr_login_check,
            interface::admin_commands::save_source_cookie,
            interface::admin_commands::clear_source_cookie,
            interface::settings_commands::get_settings,
            interface::settings_commands::update_settings,
            interface::settings_commands::get_media_tool_status,
            interface::settings_commands::install_media_tools,
            interface::render_commands::start_video_render,
            interface::diagnostics_commands::get_diagnostics,
            interface::diagnostics_commands::check_update,
            interface::download_commands::create_download,
            interface::download_commands::create_batch_download,
            interface::download_commands::cancel_download,
            interface::download_commands::retry_download,
            interface::download_commands::get_download_task,
            interface::download_commands::list_download_tasks,
            interface::download_commands::open_download_folder,
            interface::local_music_commands::get_local_music,
            interface::local_music_commands::refresh_local_music,
            interface::local_music_commands::delete_local_music,
            interface::local_music_commands::get_local_track,
            interface::local_music_commands::get_local_duplicates,
            interface::local_music_commands::precheck_downloads,
            interface::collection_commands::list_collections,
            interface::collection_commands::create_collection,
            interface::collection_commands::rename_collection,
            interface::collection_commands::delete_collection,
            interface::collection_commands::get_collection_songs,
            interface::collection_commands::add_collection_songs,
            interface::collection_commands::remove_collection_song,
            interface::collection_commands::reorder_collection_songs,
            interface::collection_commands::import_collection,
            interface::history_commands::record_playback,
            interface::history_commands::list_playback_history,
            interface::history_commands::clear_playback_history,
            interface::history_commands::list_download_records,
            interface::history_commands::clear_download_records,
            interface::mcp::chat_default_model
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| tracing::error!(event = "app_run_failed", %error));
}
