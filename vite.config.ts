import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import { readFileSync } from 'node:fs'

const packageVersion = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version
const updaterConfigured = Boolean(process.env.UPDATER_PUBLIC_KEY?.trim())

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    'import.meta.env.PACKAGE_VERSION': JSON.stringify(packageVersion),
    'import.meta.env.UPDATER_CONFIGURED': JSON.stringify(updaterConfigured),
  },
})
