import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  build: {
    target: 'ES2020',
    outDir: '../src-tauri/dist',
    emptyOutDir: true
  },
  server: {
    strictPort: true,
    port: 5173
  }
})
