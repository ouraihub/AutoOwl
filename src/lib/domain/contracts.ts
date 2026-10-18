/**
 * Mirror of the Rust IPC DTOs.
 *
 * Every type here has a `#[serde(rename_all = "camelCase")]` counterpart in
 * `src-tauri/src`. Field names and optionality are the wire contract: a
 * mismatch is a runtime bug the compiler cannot catch, so this file is the
 * single place the shapes are declared.
 */

export type SourceId = string;

/** Stable identity of a track on one platform. */
export type SongRef = {
  source: SourceId;
  scope: string;
  siteId: string;
};

export type Artist = { id?: string; name: string };

export type AlbumRef = { id?: string; name: string; source?: SourceId };

export type Song = {
  /** Serialized as `ref` on the Rust side. */
  ref: SongRef;
  title: string;
  artists: Artist[];
  album?: AlbumRef;
  durationMs?: number;
  coverUrl?: string;
};

export type SearchQuery = {
  query: string;
  page: number;
  pageSize: number;
  sources: SourceId[];
};

export type SourceCapabilities = {
  songSearch: boolean;
  playlistSearch: boolean;
  albumSearch: boolean;
  lyrics: boolean;
  cover: boolean;
  cookie: boolean;
  qrLogin: boolean;
  qualities: string[];
};

export type SourceDescriptor = {
  id: SourceId;
  capabilities: SourceCapabilities;
  authenticated: boolean;
};

/** Why one source contributed nothing to an aggregated query. */
export type SourceFailure = {
  source: SourceId;
  code: string;
  message: string;
  retryable: boolean;
};

/** Another platform carrying the same track, for source switching. */
export type AlternativeSource = {
  source: SourceId;
  ref: SongRef;
  score: number;
};

export type RankedSong = {
  song: Song;
  score: number;
  alternatives: AlternativeSource[];
};

export type AggregatedSearch = {
  items: RankedSong[];
  total?: number;
  page: number;
  pageSize: number;
  sourceErrors: SourceFailure[];
  queriedSources: number;
  respondedSources: number;
};

/** Which remote listing to search. Rust: `application::search::CollectionKind`. */
export type CollectionKind = 'playlist' | 'album';

/** Playlist vs album, as a stored/remote collection's content. */
export type CollectionType = 'playlist' | 'album';

/** A remote playlist or album. */
export type Collection = {
  id: string;
  source: SourceId;
  contentType: CollectionType;
  name: string;
  description?: string;
  creator?: string;
  coverUrl?: string;
  trackCount?: number;
};

export type CollectionQuery = {
  query: string;
  page: number;
  pageSize: number;
};

export type AggregatedCollections = {
  items: Collection[];
  total?: number;
  page: number;
  pageSize: number;
  sourceErrors: SourceFailure[];
  queriedSources: number;
  respondedSources: number;
};

export type CollectionDetail = {
  collection: Collection;
  songs: Song[];
};

/** A share link resolved to its platform, kind and tracks. */
export type ParsedShareLink = {
  source: SourceId;
  kind: CollectionKind;
  id: string;
  /** Absent for a single-song link. */
  collection?: Collection;
  songs: Song[];
};

/** Whether a stored collection is user-made or imported (read-only). */
export type StoredCollectionKind = 'user' | 'imported';

/** A locally stored collection. Rust: `StoredCollection`. */
export type StoredCollection = {
  id: string;
  kind: StoredCollectionKind;
  contentType: CollectionType;
  source: SourceId;
  externalId?: string;
  name: string;
  description?: string;
  creator?: string;
  coverUrl?: string;
  link?: string;
  trackCount: number;
  createdAt: string;
  updatedAt: string;
};

export type ImportMetadata = {
  source?: string;
  externalId?: string;
  description?: string;
  creator?: string;
  coverUrl?: string;
  link?: string;
};

/** Result of adding songs to a collection; duplicates are skipped, not errors. */
export type AddOutcome = {
  requested: number;
  added: number;
  skipped: number;
};

/** One `(songId, source)` pair, for reordering. */
export type SongKey = { songId: string; source: SourceId };

/** A resolved, directly playable stream URL. */
export type PlayableResource = {
  song: SongRef;
  url: string;
  quality: string;
  durationMs?: number;
  expiresAt?: string;
};

export type LyricLine = {
  startMs?: number;
  endMs?: number;
  text: string;
};

export type Lyrics = {
  original?: string;
  translated?: string;
  romanized?: string;
  lines: LyricLine[];
};

export type Playability = 'playable' | 'unplayable' | 'unprobeable';

/** One source's probe outcome while resolving playback. */
export type PlayabilityReport = {
  source: SourceId;
  playability: Playability;
  resource?: PlayableResource;
  detail?: string;
};

/** Playback resolution, including any automatic source switch. */
export type ResolvedPlayback = {
  resource: PlayableResource;
  servedBy: SourceId;
  requested: SourceId;
  switched: boolean;
  playability: Playability;
  attempts: PlayabilityReport[];
};

/** Result of a user-initiated source switch. */
export type SourceSwitch = {
  /** Source the user switched away from. */
  from: SourceId;
  /** Source that actually served the alternative. */
  servedBy: SourceId;
  /** Verified stream on `servedBy`; `resource.song` is the new ref to play. */
  resource: PlayableResource;
  playability: Playability;
  /** Candidates probed and rejected before the winner, best-ranked first. */
  rejected: PlayabilityReport[];
};

export type PlayerStatus = 'idle' | 'loading' | 'playing' | 'paused' | 'ended' | 'failed';

export type PlayMode = 'sequential' | 'random' | 'singleLoop';

export type PlayerState = {
  status: PlayerStatus;
  current?: Song;
  positionMs: number;
  durationMs: number;
  volume: number;
  mode: PlayMode;
  queue: Song[];
  currentIndex: number | null;
};

export type DownloadState =
  | 'queued'
  | 'resolving'
  | 'downloading'
  | 'processing'
  | 'completed'
  | 'failed'
  | 'cancelRequested'
  | 'cancelled';

export type DownloadError = {
  code: string;
  message: string;
  retryable: boolean;
  operationId: string;
};

export type DownloadTask = {
  id: string;
  song: Song;
  targetPath: string;
  state: DownloadState;
  progress: number;
  downloadedBytes: number;
  totalBytes?: number;
  speedBytesPerSecond?: number;
  /** True when the file already existed and the transfer was skipped. */
  skipped: boolean;
  /** Non-fatal note, e.g. cover or lyrics could not be attached. */
  warning?: string;
  error?: DownloadError;
  createdAt: string;
  updatedAt: string;
};

export type BatchOutcomeKind = 'completed' | 'skippedDuplicate' | 'failed' | 'cancelled';

export type BatchItemOutcome = {
  taskId: string;
  song: Song;
  targetPath: string;
  outcome: BatchOutcomeKind;
  code?: string;
  message?: string;
};

export type BatchDownloadReport = {
  items: BatchItemOutcome[];
  completed: number;
  skipped: number;
  failed: number;
  cancelled: number;
};

/** A page of rows. Rust: `domain::local_music::Page<T>`. */
export type Page<T> = {
  items: T[];
  page: number;
  pageSize: number;
  total: number;
};

/** A file in the local library. */
export type LocalTrack = {
  ref: SongRef;
  id: string;
  path: string;
  title: string;
  artists: Artist[];
  album?: AlbumRef;
  format: string;
  fileSize: number;
  modifiedAt: string;
  durationMs?: number;
  coverUrl?: string;
};

/** What a library rescan changed. */
export type RefreshReport = {
  seen: number;
  added: number;
  updated: number;
  removed: number;
};

/** One page of duplicate groups. */
export type DuplicateGroupsPage = {
  groups: LocalTrack[][];
  page: number;
  pageSize: number;
  /** Number of groups, not of tracks. */
  total: number;
  totalPages: number;
};

/** One queued song for the download precheck. */
export type PrecheckSong = {
  name: string;
  artist: string;
};

/** How much of a queue is already on disk. */
export type PrecheckReport = {
  total: number;
  skipped: number;
};

/** One persisted download history row. */
export type DownloadRecord = {
  id: string;
  title: string;
  artist: string;
  targetPath: string;
  state: string;
  progress: number;
  errorCode?: string;
  createdAt: string;
};

/** One page of download records. */
export type DownloadRecordsPage = {
  records: DownloadRecord[];
  page: number;
  pageSize: number;
  total: number;
  totalPages: number;
};

/** One play, newest first in the history modal. */
export type PlaybackEntry = {
  id: number;
  song: Song;
  playedAt: string;
};

/**
 * Rust: `domain::settings::AppSettings`.
 */
export type AppSettings = {
  downloadDir: string;
  downloadFilenameTemplate: string;
  embedDownload: boolean;
  /** Clamped to 1..5 by the backend. */
  downloadConcurrency: number;
  autoSwitchInvalidSources: boolean;
  autoCacheOnPlay: boolean;
  disableFloatingLyrics: boolean;
  /** Clamped to 10..100 by the backend. */
  pageSize: number;
  autoCheckUpdate: boolean;
  updateRepoUrl: string;
  githubProxyEnabled: boolean;
  githubProxyUrl: string;
  vgChangeCover: boolean;
  vgChangeAudio: boolean;
  vgChangeLyric: boolean;
  vgExportVideo: boolean;
};

/** Cookie/QR state for one platform. */
export type AuthStatus = {
  source: SourceId;
  configured: boolean;
  qrSupported: boolean;
};

/** Lifecycle of one QR login session; mirrors the Rust `QrStatus`. */
export type QrStatus = 'pending' | 'scanned' | 'confirmed' | 'expired' | 'failed';

/** An open QR login session. `imageUrl` is a ready-to-render data URL. */
export type QrLoginSession = {
  source: SourceId;
  /** Opaque token echoed back when polling. */
  key: string;
  /** `data:image/png;base64,...` */
  imageUrl: string;
  /** Unix seconds. */
  expiresAt: number;
};

/** One poll result. The backend persists the cookie itself on `confirmed`. */
export type QrLoginResult = {
  source: SourceId;
  status: QrStatus;
  message: string;
  cookie?: string;
};

/** ffmpeg/ffprobe availability, with the reason when a tool is missing. */
export type MediaToolStatus = {
  ffmpeg: boolean;
  ffprobe: boolean;
  ffmpegPath?: string;
  ffprobePath?: string;
  ffmpegError?: string;
  ffprobeError?: string;
};

export type Diagnostics = {
  appVersion: string;
  platform: string;
  arch: string;
  dataDir: string;
  logDir: string;
  downloadDir: string;
  databaseReady: boolean;
};

export type UpdateInfo = {
  currentVersion: string;
  latestVersion?: string;
  releaseUrl?: string;
  available: boolean;
};

export type RenderState = 'queued' | 'preparing' | 'rendering' | 'completed' | 'failed' | 'cancelled';

export type RenderTask = {
  id: string;
  state: RenderState;
  progress: number;
  outputPath?: string;
  errorCode?: string;
};

/** Shape of a rejected `invoke`. Rust: `error::IpcError`. */
export type AppError = {
  code: string;
  message: string;
  retryable: boolean;
  operationId: string;
};

/** Emitted as `download:changed` on every task state transition. */
export type DownloadChangedEvent = { task: DownloadTask };

/** Emitted as `download:progress` while bytes move. */
export type DownloadProgressEvent = {
  id: string;
  downloadedBytes: number;
  totalBytes?: number;
  progress: number;
  speedBytesPerSecond?: number;
};

/** Emitted as `player:state` on every player mutation. */
export type PlayerChangedEvent = PlayerState;
