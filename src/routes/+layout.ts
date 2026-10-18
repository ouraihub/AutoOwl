/**
 * The app ships as a Tauri WebView SPA: `adapter-static` emits the shell and
 * every route hydrates in the client. SSR must stay off because pages read
 * `page.url.searchParams` and invoke Tauri commands at init, neither of which
 * exists on a server.
 */
export const prerender = true;
export const ssr = false;
