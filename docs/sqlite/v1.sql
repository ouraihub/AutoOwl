CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS credentials (
  source TEXT PRIMARY KEY,
  encrypted_value BLOB NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS download_tasks (
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  scope TEXT NOT NULL,
  site_id TEXT NOT NULL,
  title TEXT NOT NULL,
  artist TEXT,
  target_path TEXT NOT NULL,
  state TEXT NOT NULL,
  progress REAL NOT NULL DEFAULT 0,
  error_code TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_download_tasks_state ON download_tasks(state);
CREATE INDEX IF NOT EXISTS idx_download_tasks_created_at ON download_tasks(created_at DESC);

CREATE TABLE IF NOT EXISTS collections (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  content_type TEXT NOT NULL,
  source TEXT NOT NULL,
  external_id TEXT,
  name TEXT NOT NULL,
  description TEXT,
  creator TEXT,
  cover_url TEXT,
  link TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_collections_external
  ON collections(source, external_id, content_type);

CREATE TABLE IF NOT EXISTS collection_songs (
  collection_id TEXT NOT NULL,
  song_id TEXT NOT NULL,
  source TEXT NOT NULL,
  position INTEGER NOT NULL,
  song_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(collection_id, song_id, source),
  FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_collection_songs_position
  ON collection_songs(collection_id, position);

CREATE TABLE IF NOT EXISTS local_music_tracks (
  id TEXT PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  fingerprint TEXT,
  title TEXT NOT NULL,
  artist TEXT,
  album TEXT,
  format TEXT NOT NULL,
  file_size INTEGER NOT NULL,
  modified_at TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  indexed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_local_music_title_artist
  ON local_music_tracks(title, artist);

CREATE TABLE IF NOT EXISTS playback_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  song_json TEXT NOT NULL,
  played_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_playback_history_played_at
  ON playback_history(played_at DESC);

CREATE TABLE IF NOT EXISTS download_dedup (
  source TEXT NOT NULL,
  scope TEXT NOT NULL,
  site_id TEXT NOT NULL,
  target_path TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(source, scope, site_id, target_path)
);
