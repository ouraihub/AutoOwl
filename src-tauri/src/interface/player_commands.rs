use crate::domain::player::{PlayMode, PlayerState};
use crate::domain::source::Song;
use crate::error::IpcError;
use crate::state::AppState;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub fn player_load(
    app: AppHandle,
    state: State<'_, AppState>,
    song: Song,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.load(song);
        Ok(())
    })
}

#[tauri::command]
pub fn player_play(app: AppHandle, state: State<'_, AppState>) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.play();
        Ok(())
    })
}

#[tauri::command]
pub fn player_pause(app: AppHandle, state: State<'_, AppState>) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.pause();
        Ok(())
    })
}

#[tauri::command]
pub fn player_next(app: AppHandle, state: State<'_, AppState>) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.next();
        Ok(())
    })
}

#[tauri::command]
pub fn player_previous(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.previous();
        Ok(())
    })
}

#[tauri::command]
pub fn player_seek(
    app: AppHandle,
    state: State<'_, AppState>,
    position_ms: u64,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.seek(position_ms);
        Ok(())
    })
}

/// Reports the real container duration once the player has decoded the stream.
///
/// Search metadata seeds an estimate at load time, but some sources report no
/// length at all; without this the backend would treat the track as unknown and
/// could not clamp seeks.
#[tauri::command]
pub fn player_report_duration(
    app: AppHandle,
    state: State<'_, AppState>,
    duration_ms: u64,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.set_duration(duration_ms);
        Ok(())
    })
}

#[tauri::command]
pub fn player_set_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: PlayMode,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.set_mode(mode);
        Ok(())
    })
}

#[tauri::command]
pub fn player_get_state(state: State<'_, AppState>) -> Result<PlayerState, IpcError> {
    Ok(lock_player(&state)?.clone())
}

#[tauri::command]
pub fn player_set_volume(
    app: AppHandle,
    state: State<'_, AppState>,
    volume: f32,
) -> Result<PlayerState, IpcError> {
    if !(0.0..=1.0).contains(&volume) {
        return Err(error("INVALID_ARGUMENT", "音量必须在 0 到 1 之间", false));
    }
    mutate(&app, &state, |player| {
        player.volume = volume;
        Ok(())
    })
}

/// Replaces the queue and starts at `start_index`.
#[tauri::command]
pub fn player_set_queue(
    app: AppHandle,
    state: State<'_, AppState>,
    songs: Vec<Song>,
    start_index: usize,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.set_queue(songs, start_index);
        Ok(())
    })
}

#[tauri::command]
pub fn player_remove_from_queue(
    app: AppHandle,
    state: State<'_, AppState>,
    index: usize,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        if player.remove_from_queue(index) {
            Ok(())
        } else {
            Err(error("INVALID_ARGUMENT", "播放列表索引超出范围", false))
        }
    })
}

#[tauri::command]
pub fn player_clear_queue(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        player.clear_queue();
        Ok(())
    })
}

#[tauri::command]
pub fn player_move_in_queue(
    app: AppHandle,
    state: State<'_, AppState>,
    from: usize,
    to: usize,
) -> Result<PlayerState, IpcError> {
    mutate(&app, &state, |player| {
        if player.move_in_queue(from, to) {
            Ok(())
        } else {
            Err(error("INVALID_ARGUMENT", "播放列表索引超出范围", false))
        }
    })
}

/// Applies a mutation under the lock and emits `player:state` once.
///
/// The lock is released before emitting so an event listener calling back into a
/// player command cannot deadlock.
fn mutate<F>(
    app: &AppHandle,
    state: &State<'_, AppState>,
    apply: F,
) -> Result<PlayerState, IpcError>
where
    F: FnOnce(&mut PlayerState) -> Result<(), IpcError>,
{
    let snapshot = {
        let mut player = lock_player(state)?;
        apply(&mut player)?;
        player.clone()
    };
    if let Err(source) = app.emit("player:state", &snapshot) {
        tracing::warn!(event = "player_event_failed", %source);
    }
    Ok(snapshot)
}

fn lock_player<'a>(
    state: &'a State<'_, AppState>,
) -> Result<std::sync::MutexGuard<'a, PlayerState>, IpcError> {
    state
        .player
        .lock()
        .map_err(|_| error("STATE_UNAVAILABLE", "播放器状态暂时不可用", true))
}

fn error(code: &str, message: &str, retryable: bool) -> IpcError {
    IpcError::new(code, message, retryable, "player_state".to_owned())
}
