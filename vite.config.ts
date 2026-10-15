import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [vue()],
  base: './',
  resolve: { preserveSymlinks: true },
  server: {
    host: '127.0.0.1',
    watch: { ignored: ['**/test-output/**', '**/release/**', '**/src-tauri/**', '**/data/**'] },
  },
  build: { outDir: 'dist', emptyOutDir: true },
});
