import js from '@eslint/js'
import { defineConfig } from 'eslint/config'
import tseslint from 'typescript-eslint'
import noUiLiterals from './scripts/eslint/no-ui-literals.mjs'

export default defineConfig([
  { ignores: ['dist/**', 'node_modules/**', 'src-tauri/target/**'] },
  {
    files: ['src/**/*.{ts,tsx}'],
    extends: [js.configs.recommended, tseslint.configs.recommended],
    plugins: { 'flux-ui': { rules: { 'no-ui-literals': noUiLiterals } } },
    rules: { 'flux-ui/no-ui-literals': 'error' },
  },
])
