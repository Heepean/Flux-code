import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { readFileSync } from 'node:fs';

const appVersion = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version as string;

export default defineConfig({ plugins:[react()], define:{__APP_VERSION__:JSON.stringify(appVersion)}, clearScreen:false, server:{host:'127.0.0.1',port:1420,strictPort:true,watch:{ignored:['**/src-tauri/target/**']}}, envPrefix:['VITE_','TAURI_'], build:{target:process.env.TAURI_ENV_PLATFORM==='windows'?'chrome105':'safari13'} });
