import { redirect } from '@sveltejs/kit';

/** The shell has no dedicated home surface: search is the entry point. */
export function load(): never {
  redirect(307, '/search');
}
