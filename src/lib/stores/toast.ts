import type { AppError } from '$lib/domain/contracts';
import { toast } from 'svelte-sonner';

/**
 * Single import site for notifications, backed by `svelte-sonner`.
 * Requires `<Toaster />` to be mounted once in the app shell.
 */
export const notify = {
  success(message: string, description?: string): void {
    toast.success(message, { description });
  },

  error(message: string, description?: string): void {
    toast.error(message, { description });
  },

  info(message: string, description?: string): void {
    toast.info(message, { description });
  },

  /** Show an `AppError`, using its code as the description for support. */
  fromError(error: AppError, fallback = '操作失败'): void {
    toast.error(error.message || fallback, { description: `错误码：${error.code}` });
  },

  loading(message: string): string | number {
    return toast.loading(message);
  },

  dismiss(id?: string | number): void {
    toast.dismiss(id);
  }
};

export { toast };
