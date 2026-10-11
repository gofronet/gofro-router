import { expect, test } from 'bun:test';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';

const dist = new URL('./dist/', import.meta.url);
const read = (name: string) => readFileSync(new URL(name, dist));
const bundles = ['app.css', 'app.js', 'chart.js'];

test('built UI has versioned references, matching hashes and gzip copies', () => {
  expect(readdirSync(dist).sort()).toEqual([
    'index.html', ...bundles.flatMap(name => [name, `${name}.gz`, `${name}.sha256`]),
  ].sort());

  for (const name of bundles) {
    const contents = read(name);
    const hash = createHash('sha256').update(contents).digest('hex');
    expect(read(`${name}.sha256`).toString()).toBe(hash);
    expect(gunzipSync(read(`${name}.gz`))).toEqual(contents);
    const source = read(name === 'chart.js' ? 'app.js' : 'index.html').toString();
    expect(source).toContain(`${name}?v=${hash}`);
    expect(source).not.toMatch(new RegExp(`${name.replace('.', '\\.')}["']`));
  }
});
