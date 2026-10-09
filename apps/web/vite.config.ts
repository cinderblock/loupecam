import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// During `bun run dev`, API and stream requests go to a running `loupecam serve`.
const target = process.env.LOUPECAM_SERVER ?? 'http://127.0.0.1:8080'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target, ws: true },
      '/stream.mjpg': target,
      '/snapshot.jpg': target,
    },
  },
})
