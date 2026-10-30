import {build} from 'esbuild';
import {resolve} from 'node:path';
import {execFileSync} from 'node:child_process';
const root=resolve(import.meta.dirname,'..');
await import('./build-brand.mjs');
execFileSync(process.execPath,[resolve(root,'node_modules/@tailwindcss/cli/dist/index.mjs'),'-i',resolve(root,'frontend/theme.css'),'-o',resolve(root,'public/assets/theme.css'),'--minify'],{cwd:root,stdio:'inherit'});
await build({entryPoints:[resolve(root,'frontend/main.jsx')],bundle:true,minify:true,format:'esm',platform:'browser',target:['es2022'],jsx:'automatic',outfile:resolve(root,'public/assets/aiconnect.js'),assetNames:'fonts/[name]-[hash]',loader:{'.woff2':'file','.woff':'file'},define:{'process.env.NODE_ENV':'"production"'},legalComments:'eof',logLevel:'info'});
