import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const assets = new URL("../assets/", import.meta.url);

function fingerprint(name, contents) {
  const hash = createHash("sha256").update(contents).digest("hex");
  writeFileSync(new URL(`${name}.gz`, assets), gzipSync(contents, { level: 9 }));
  writeFileSync(new URL(`${name}.sha256`, assets), hash);
  return hash;
}

function replaceOne(contents, search, replacement) {
  const first = contents.indexOf(search);
  if (first < 0 || contents.indexOf(search, first + search.length) >= 0) {
    throw new Error(`expected one ${search} reference`);
  }
  return contents.replace(search, replacement);
}

const chart = readFileSync(new URL("chart.js", assets));
const chartHash = fingerprint("chart.js", chart);
const appPath = new URL("app.js", assets);
const appSource = readFileSync(appPath, "utf8");
const app = replaceOne(appSource, "./chart.js", `./chart.js?v=${chartHash}`);
writeFileSync(appPath, app);

const appHash = fingerprint("app.js", Buffer.from(app));
const cssHash = fingerprint("app.css", readFileSync(new URL("app.css", assets)));
const htmlPath = new URL("index.html", assets);
const htmlSource = readFileSync(htmlPath, "utf8");
const html = replaceOne(
  replaceOne(htmlSource, 'src="/app.js"', `src="/app.js?v=${appHash}"`),
  'href="/app.css"',
  `href="/app.css?v=${cssHash}"`,
);
writeFileSync(htmlPath, html);
