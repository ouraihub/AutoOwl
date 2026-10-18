use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

use super::source::Song;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlayerStatus {
    Idle,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlayMode {
    Sequential,
    Random,
    SingleLoop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    pub status: PlayerStatus,
    /// Omitted when idle: the TS contract is `current?: Song`, and `null` would
    /// slip past its `=== undefined` checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<Song>,
    pub position_ms: u64,
    /// Length of the current track in milliseconds, where `0` means *unknown*:
    /// not every source reports a length, and the real container duration is
    /// only known once the player has decoded the stream.
    pub duration_ms: u64,
    pub volume: f32,
    pub mode: PlayMode,
    pub queue: Vec<Song>,
    pub current_index: Option<usize>,
    /// Visiting order for [`PlayMode::Random`]: a permutation of the queue
    /// indices, so a pass plays every entry once instead of coin-flipping and
    /// replaying songs. Internal bookkeeping, never sent to the UI.
    #[serde(skip)]
    shuffle_order: Vec<usize>,
    #[serde(skip)]
    shuffle_cursor: usize,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: PlayerStatus::Idle,
            current: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 1.0,
            mode: PlayMode::Sequential,
            queue: Vec::new(),
            current_index: None,
            shuffle_order: Vec::new(),
            shuffle_cursor: 0,
        }
    }
}

impl PlayerState {
    pub fn load(&mut self, song: Song) {
        let index = self
            .queue
            .iter()
            .position(|item| same_song(item, &song))
            .unwrap_or_else(|| {
                self.queue.push(song.clone());
                self.queue.len() - 1
            });
        self.current = Some(song);
        self.adopt_current_duration();
        self.current_index = Some(index);
        self.position_ms = 0;
        self.status = PlayerStatus::Loading;
        self.resync_shuffle();
    }

    pub fn play(&mut self) {
        if self.current.is_some() {
            self.status = PlayerStatus::Playing;
        }
    }

    pub fn pause(&mut self) {
        if self.status == PlayerStatus::Playing {
            self.status = PlayerStatus::Paused;
        }
    }

    pub fn end(&mut self) {
        self.status = PlayerStatus::Ended;
    }

    /// Clamps to the track length, except when [`PlayerState::duration_ms`] is
    /// `0`. That value means the length is unknown, and clamping against it
    /// would pin every seek to the start of the track.
    pub fn seek(&mut self, position_ms: u64) {
        self.position_ms = if self.duration_ms == 0 {
            position_ms
        } else {
            position_ms.min(self.duration_ms)
        };
    }

    /// Records the real container duration once the player has decoded it,
    /// superseding the metadata estimate seeded when the track was loaded.
    pub fn set_duration(&mut self, duration_ms: u64) {
        self.duration_ms = duration_ms;
    }

    pub fn set_mode(&mut self, mode: PlayMode) {
        self.mode = mode;
        if mode == PlayMode::Random {
            self.resync_shuffle();
        }
    }

    pub fn next(&mut self) {
        self.move_by(1);
    }

    pub fn previous(&mut self) {
        self.move_by(-1);
    }

    /// Replaces the whole queue and starts playing at `start_index`, which is
    /// clamped to the queue instead of rejected so a stale UI index still plays.
    pub fn set_queue(&mut self, songs: Vec<Song>, start_index: usize) {
        self.queue = songs;
        if self.queue.is_empty() {
            self.reset_playback();
            return;
        }
        let index = start_index.min(self.queue.len() - 1);
        self.current_index = Some(index);
        self.current = self.queue.get(index).cloned();
        self.adopt_current_duration();
        self.position_ms = 0;
        self.status = PlayerStatus::Loading;
        self.rebuild_shuffle();
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
        self.reset_playback();
    }

    /// Removes one entry; `false` when the index is out of range.
    ///
    /// Playback stays on the same song unless that song was the one removed, in
    /// which case the slot is reused so the following track becomes current.
    pub fn remove_from_queue(&mut self, index: usize) -> bool {
        if index >= self.queue.len() {
            return false;
        }
        self.queue.remove(index);
        if self.queue.is_empty() {
            self.reset_playback();
            return true;
        }
        match self.current_index {
            Some(current) if current == index => {
                let next = current.min(self.queue.len() - 1);
                self.current_index = Some(next);
                self.current = self.queue.get(next).cloned();
                self.adopt_current_duration();
                self.position_ms = 0;
                self.status = PlayerStatus::Loading;
            }
            Some(current) if current > index => self.current_index = Some(current - 1),
            _ => {}
        }
        self.rebuild_shuffle();
        true
    }

    /// Reorders the queue, keeping the playing song current and untouched;
    /// `false` when either index is out of range.
    pub fn move_in_queue(&mut self, from: usize, to: usize) -> bool {
        let len = self.queue.len();
        if from >= len || to >= len {
            return false;
        }
        if from == to {
            return true;
        }
        let song = self.queue.remove(from);
        self.queue.insert(to, song);
        if let Some(current) = self.current_index {
            let moved = if current == from {
                to
            } else {
                // The removal shifts entries after `from` down, the insertion
                // shifts entries at or after `to` back up.
                let mut index = current;
                if from < index {
                    index -= 1;
                }
                if to <= index {
                    index += 1;
                }
                index
            };
            self.current_index = Some(moved);
            self.current = self.queue.get(moved).cloned();
        }
        self.rebuild_shuffle();
        true
    }

    fn move_by(&mut self, offset: isize) {
        if self.queue.is_empty() {
            return;
        }
        let current = self.current_index.unwrap_or(0);
        let next = match self.mode {
            PlayMode::SingleLoop => current,
            PlayMode::Sequential => {
                (current as isize + offset).rem_euclid(self.queue.len() as isize) as usize
            }
            PlayMode::Random => self.advance_random(offset),
        };
        self.current_index = Some(next);
        self.current = self.queue.get(next).cloned();
        self.adopt_current_duration();
        self.position_ms = 0;
        self.status = PlayerStatus::Loading;
    }

    /// Walks the shuffled pass; running past either end starts a fresh pass.
    fn advance_random(&mut self, offset: isize) -> usize {
        self.resync_shuffle();
        let len = self.queue.len();
        let cursor = self.shuffle_cursor as isize + offset;
        if cursor < 0 || cursor as usize >= len {
            self.start_new_pass();
        } else {
            self.shuffle_cursor = cursor as usize;
        }
        self.shuffle_order
            .get(self.shuffle_cursor)
            .copied()
            .unwrap_or(0)
    }

    /// Reshuffles and avoids opening the new pass with the song that just
    /// finished, which would sound like a stutter rather than a fresh pass.
    fn start_new_pass(&mut self) {
        let previous = self.current_index;
        let len = self.queue.len();
        self.shuffle_order = shuffled_indices(len);
        self.shuffle_cursor = 0;
        if len > 1 && self.shuffle_order.first().copied() == previous {
            self.shuffle_order.swap(0, len - 1);
        }
    }

    /// Keeps the pass usable after queue edits, rebuilding it only when it no
    /// longer describes the queue.
    fn resync_shuffle(&mut self) {
        if is_permutation(&self.shuffle_order, self.queue.len()) {
            if let Some(at) = self.cursor_of_current() {
                self.shuffle_cursor = at;
            }
            return;
        }
        self.rebuild_shuffle();
    }

    /// Starts a pass anchored on the playing song, so the remaining entries are
    /// still each visited once before the next reshuffle.
    fn rebuild_shuffle(&mut self) {
        self.shuffle_order = shuffled_indices(self.queue.len());
        self.shuffle_cursor = 0;
        if let Some(at) = self.cursor_of_current() {
            self.shuffle_order.swap(0, at);
        }
    }

    fn cursor_of_current(&self) -> Option<usize> {
        let current = self.current_index?;
        self.shuffle_order.iter().position(|&at| at == current)
    }

    fn reset_playback(&mut self) {
        self.current = None;
        self.current_index = None;
        self.position_ms = 0;
        self.duration_ms = 0;
        self.status = PlayerStatus::Idle;
        self.shuffle_order.clear();
        self.shuffle_cursor = 0;
    }

    /// Re-seeds the duration from the new current song's metadata. Called from
    /// every track change so a source that reports no length resets to unknown
    /// instead of inheriting the previous track's duration and clamping seeks
    /// against it.
    fn adopt_current_duration(&mut self) {
        self.duration_ms = self
            .current
            .as_ref()
            .and_then(|song| song.duration_ms)
            .unwrap_or(0);
    }
}

fn same_song(left: &Song, right: &Song) -> bool {
    left.reference.source == right.reference.source
        && left.reference.scope == right.reference.scope
        && left.reference.site_id == right.reference.site_id
}

fn shuffled_indices(len: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    order.shuffle(&mut rand::thread_rng());
    order
}

fn is_permutation(order: &[usize], len: usize) -> bool {
    if order.len() != len {
        return false;
    }
    let mut seen = vec![false; len];
    order.iter().all(|&index| match seen.get_mut(index) {
        Some(slot) if !*slot => {
            *slot = true;
            true
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::{PlayMode, PlayerState, PlayerStatus};
    use crate::domain::source::{Artist, Song, SongRef};
    use std::collections::HashSet;

    fn song(id: &str) -> Song {
        Song {
            reference: SongRef {
                source: "test".into(),
                scope: "song".into(),
                site_id: id.into(),
            },
            title: id.into(),
            artists: vec![Artist {
                id: None,
                name: "Artist".into(),
            }],
            album: None,
            duration_ms: None,
            cover_url: None,
        }
    }

    fn queue_of(ids: &[&str]) -> Vec<Song> {
        ids.iter().map(|id| song(id)).collect()
    }

    fn song_lasting(id: &str, duration_ms: u64) -> Song {
        Song {
            duration_ms: Some(duration_ms),
            ..song(id)
        }
    }

    fn current_title(state: &PlayerState) -> Option<&str> {
        state.current.as_ref().map(|item| item.title.as_str())
    }

    #[test]
    fn play_requires_loaded_song() {
        let mut state = PlayerState::default();
        state.play();
        assert_eq!(state.status, PlayerStatus::Idle);
    }

    #[test]
    fn sequential_queue_wraps_and_single_loop_stays() {
        let mut state = PlayerState::default();
        state.load(song("one"));
        state.load(song("two"));
        state.next();
        assert_eq!(current_title(&state), Some("one"));
        state.previous();
        assert_eq!(current_title(&state), Some("two"));
        state.set_mode(PlayMode::SingleLoop);
        state.next();
        assert_eq!(current_title(&state), Some("two"));
        state.previous();
        assert_eq!(current_title(&state), Some("two"));
    }

    #[test]
    fn random_visits_every_entry_once_before_repeating() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b", "c", "d", "e"]), 0);
        state.set_mode(PlayMode::Random);
        let mut seen = HashSet::new();
        seen.insert(state.current_index.expect("queue starts playing"));
        for _ in 1..5 {
            state.next();
            let index = state.current_index.expect("random mode keeps an index");
            assert!(seen.insert(index), "index {index} repeated within one pass");
        }
        assert_eq!(seen.len(), 5, "a full pass must cover the whole queue");
    }

    #[test]
    fn random_starts_a_fresh_pass_without_replaying_the_same_song() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b", "c", "d"]), 0);
        state.set_mode(PlayMode::Random);
        for _ in 0..3 {
            state.next();
        }
        let last = state.current_index.expect("pass ends on a song");
        state.next();
        assert_ne!(
            state.current_index,
            Some(last),
            "a new pass must not reopen with the song that just played"
        );
    }

    #[test]
    fn removing_another_entry_keeps_playing_the_same_song() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b", "c"]), 2);
        assert!(state.remove_from_queue(0));
        assert_eq!(current_title(&state), Some("c"));
        assert_eq!(state.current_index, Some(1));
        assert_eq!(state.queue.len(), 2);
    }

    #[test]
    fn removing_the_playing_entry_advances_to_the_next_song() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b", "c"]), 1);
        assert!(state.remove_from_queue(1));
        assert_eq!(current_title(&state), Some("c"));
        assert_eq!(state.current_index, Some(1));
    }

    #[test]
    fn removing_the_last_playing_entry_falls_back_to_the_new_tail() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b"]), 1);
        assert!(state.remove_from_queue(1));
        assert_eq!(current_title(&state), Some("a"));
        assert_eq!(state.current_index, Some(0));
    }

    #[test]
    fn removing_out_of_range_is_rejected() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a"]), 0);
        assert!(!state.remove_from_queue(3));
        assert_eq!(state.queue.len(), 1);
    }

    #[test]
    fn moving_entries_keeps_current_index_on_the_same_song() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b", "c", "d"]), 2);
        // Move an earlier entry past the playing song: "c" shifts down one slot.
        assert!(state.move_in_queue(0, 3));
        assert_eq!(current_title(&state), Some("c"));
        assert_eq!(state.current_index, Some(1));
        // Moving the playing song itself follows it to the destination.
        assert!(state.move_in_queue(1, 3));
        assert_eq!(current_title(&state), Some("c"));
        assert_eq!(state.current_index, Some(3));
        // A later entry moved before the playing song pushes it up one slot.
        assert!(state.move_in_queue(2, 0));
        assert_eq!(current_title(&state), Some("c"));
        assert_eq!(state.current_index, Some(3));
    }

    #[test]
    fn move_out_of_range_is_rejected_and_queue_is_untouched() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b"]), 0);
        assert!(!state.move_in_queue(0, 5));
        assert_eq!(state.queue[0].title, "a");
        assert_eq!(state.queue[1].title, "b");
    }

    #[test]
    fn set_queue_clamps_a_stale_start_index() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b"]), 9);
        assert_eq!(current_title(&state), Some("b"));
        assert_eq!(state.current_index, Some(1));
        assert_eq!(state.status, PlayerStatus::Loading);
    }

    #[test]
    fn clearing_the_queue_returns_to_idle() {
        let mut state = PlayerState::default();
        state.set_queue(queue_of(&["a", "b"]), 0);
        state.clear_queue();
        assert_eq!(state.status, PlayerStatus::Idle);
        assert!(state.current.is_none());
        assert_eq!(state.current_index, None);
        assert!(state.queue.is_empty());
    }

    #[test]
    fn empty_queue_ignores_navigation() {
        let mut state = PlayerState::default();
        state.set_mode(PlayMode::Random);
        state.next();
        state.previous();
        assert_eq!(state.current_index, None);
        assert_eq!(state.status, PlayerStatus::Idle);
    }

    #[test]
    fn seeking_with_an_unknown_duration_keeps_the_requested_position() {
        let mut state = PlayerState::default();
        state.load(song("unknown"));
        assert_eq!(
            state.duration_ms, 0,
            "a song without metadata stays unknown"
        );
        state.seek(60_000);
        assert_eq!(
            state.position_ms, 60_000,
            "an unknown duration must not clamp the seek to the start of the track"
        );
    }

    #[test]
    fn seeking_past_a_known_duration_clamps_to_the_end() {
        let mut state = PlayerState::default();
        state.load(song("known"));
        state.set_duration(180_000);
        state.seek(600_000);
        assert_eq!(state.position_ms, 180_000);
        state.seek(60_000);
        assert_eq!(state.position_ms, 60_000, "an in-range seek is untouched");
    }

    #[test]
    fn load_seeds_the_duration_from_the_song_metadata() {
        let mut state = PlayerState::default();
        state.load(song_lasting("timed", 240_000));
        assert_eq!(state.duration_ms, 240_000);
        // Switching to a song that reports no length must reset to unknown rather
        // than clamp against the previous track's duration.
        state.load(song("untimed"));
        assert_eq!(state.duration_ms, 0);
    }
}
