import { describe, expect, it } from 'vitest';
import type { AppError } from '$lib/domain/contracts';
import { toAppError } from '$lib/services/errors';

describe('toAppError', () => {
  it('passes a serialized IpcError through untouched', () => {
    const rejected: AppError = {
      code: 'SOURCE_UNAVAILABLE',
      message: '网易云音乐暂时无法访问',
      retryable: true,
      operationId: 'op-42'
    };

    expect(toAppError(rejected)).toBe(rejected);
  });

  it('keeps a falsy-but-valid retryable flag rather than treating it as missing', () => {
    const rejected: AppError = {
      code: 'NOT_FOUND',
      message: '歌曲不存在',
      retryable: false,
      operationId: ''
    };

    expect(toAppError(rejected)).toBe(rejected);
  });

  it('synthesizes a fallback from a thrown Error, preserving its message', () => {
    expect(toAppError(new Error('boom'))).toEqual({
      code: 'IPC_UNKNOWN',
      message: 'boom',
      retryable: false,
      operationId: ''
    });
  });

  it('synthesizes a fallback from a thrown string', () => {
    expect(toAppError('transport closed')).toEqual({
      code: 'IPC_UNKNOWN',
      message: 'transport closed',
      retryable: false,
      operationId: ''
    });
  });

  it('synthesizes a fallback for an object that only partially matches IpcError', () => {
    expect(toAppError({ code: 'PARTIAL', message: 'no retryable flag' }).code).toBe('IPC_UNKNOWN');
  });

  it('synthesizes a fallback for null', () => {
    expect(toAppError(null)).toEqual({
      code: 'IPC_UNKNOWN',
      message: 'null',
      retryable: false,
      operationId: ''
    });
  });
});
