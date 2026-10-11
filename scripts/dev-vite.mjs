import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const vite = resolve(root,'node_modules/vite/bin/vite.js');
const child = spawn(process.execPath, [vite,'--host','127.0.0.1'], { cwd:root, stdio:'inherit', windowsHide:true });
child.on('exit', code => process.exit(code ?? 0));
process.on('SIGINT', () => child.kill('SIGINT'));
