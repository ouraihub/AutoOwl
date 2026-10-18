import type { AppError } from '$lib/domain/contracts';

/**
 * A rejected Tauri `invoke` surfaces the serialized Rust `IpcError` as a plain
 * object. Anything else (a thrown `Error`, a string, a transport failure) has to
 * be widened into the same shape so callers only ever handle one error type.
 */
function isIpcError(value: unknown): value is AppError {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.code === 'string' &&
    typeof candidate.message === 'string' &&
    typeof candidate.retryable === 'boolean'
  );
}

/** Normalize any thrown/rejected value into the frozen `AppError` shape. */
export function toAppError(error: unknown): AppError {
  if (isIpcError(error)) return error;

  return {
    code: 'IPC_UNKNOWN',
    message: error instanceof Error ? error.message : String(error),
    retryable: false,
    operationId: ''
  };
}
