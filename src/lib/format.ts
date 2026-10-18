/**
 * Display formatters shared by the download queue, the local library and the
 * player. Every function is total: unknown, absent and nonsensical inputs get a
 * placeholder, so `NaN`/`undefined` can never reach the DOM.
 */

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;

/** True only for a real, non-negative, finite number. */
function isSize(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

/**
 * Human-readable byte count, e.g. `0 B`, `931 KB`, `4.7 GB`.
 *
 * Bytes stay integral (no `0.5 B`); larger units keep one decimal unless the
 * value is already ≥ 100, where a decimal is just noise.
 */
export function formatBytes(bytes?: number, placeholder = '-'): string {
  if (!isSize(bytes)) return placeholder;
  if (bytes < 1024) return `${Math.round(bytes)} B`;

  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }

  const digits = value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${BYTE_UNITS[unit]}`;
}

/** Transfer rate, e.g. `1.2 MB/s`. Empty string when the backend sent no rate. */
export function formatSpeed(bytesPerSecond?: number): string {
  if (!isSize(bytesPerSecond) || bytesPerSecond === 0) return '';
  return `${formatBytes(bytesPerSecond)}/s`;
}

/** `mm:ss`, widening to `h:mm:ss` past an hour. `--:--` when the duration is unknown. */
export function formatDuration(ms?: number, placeholder = '--:--'): string {
  if (!isSize(ms)) return placeholder;

  const total = Math.floor(ms / 1000);
  const seconds = total % 60;
  const minutes = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const pad = (n: number) => String(n).padStart(2, '0');

  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`;
}

/** Progress ratio clamped to 0..1, so a stale or bogus frame cannot overflow the bar. */
export function clampRatio(ratio?: number): number {
  if (typeof ratio !== 'number' || !Number.isFinite(ratio)) return 0;
  return Math.min(1, Math.max(0, ratio));
}

/** Short local timestamp; falls back to the placeholder for absent/unparseable input. */
export function formatDateTime(iso?: string, placeholder = '-'): string {
  if (!iso) return placeholder;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return placeholder;

  return at.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  });
}
