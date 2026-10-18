import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  // aether-ui ships .svelte/.svelte.ts sources; the dev optimizer cannot parse
  // them (AutoNote 82e9e72), so compile it like local source instead.
  optimizeDeps: { exclude: ['@ouraihub/aether-ui'] }
});
