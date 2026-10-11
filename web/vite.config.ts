import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';

const dist = new URL('./dist/', import.meta.url);

function fingerprint(name: string, contents: Uint8Array) {
  const hash = createHash('sha256').update(contents).digest('hex');
  writeFileSync(new URL(`${name}.gz`, dist), gzipSync(contents, { level: 9 }));
  writeFileSync(new URL(`${name}.sha256`, dist), hash);
  return hash;
}

function replaceOne(contents: string, search: string, replacement: string) {
  const first = contents.indexOf(search);
  if (first < 0 || contents.indexOf(search, first + search.length) >= 0) {
    throw new Error(`expected one ${search} reference`);
  }
  return contents.replace(search, replacement);
}

export default defineConfig({
  base: '/',
  plugins: [tailwindcss(), svelte(), {
    name: 'finalize-assets',
    apply: 'build',
    closeBundle() {
      const chartHash = fingerprint('chart.js', readFileSync(new URL('chart.js', dist)));
      const appPath = new URL('app.js', dist);
      const app = replaceOne(readFileSync(appPath, 'utf8'), './chart.js', `./chart.js?v=${chartHash}`);
      writeFileSync(appPath, app);
      const appHash = fingerprint('app.js', Buffer.from(app));
      const cssHash = fingerprint('app.css', readFileSync(new URL('app.css', dist)));
      const htmlPath = new URL('index.html', dist);
      const html = replaceOne(
        replaceOne(readFileSync(htmlPath, 'utf8'), 'src="/app.js"', `src="/app.js?v=${appHash}"`),
        'href="/app.css"', `href="/app.css?v=${cssHash}"`,
      );
      writeFileSync(htmlPath, html);
    },
  }],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        entryFileNames: 'app.js',
        chunkFileNames: '[name].js',
        assetFileNames: (asset) => asset.name?.endsWith('.css') ? 'app.css' : '[name][extname]',
        manualChunks: { chart: ['chart.js'] }
      }
    }
  }
});
