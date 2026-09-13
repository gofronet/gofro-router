// Run after `bun run build`: PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs bun check-ui.mjs
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { gunzipSync } from "node:zlib";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const assets = new URL("../assets/", import.meta.url);
// Reserved origin, entirely fulfilled in-process. No device, dev server, or network fallback.
const origin = "https://gofro.test:8443";
const key = (n) => Buffer.from(String.fromCharCode(97 + n).repeat(32)).toString("base64");
const serverKey = key(20);
const otherKey = key(21);
const profile = `[Interface]\nPrivateKey = ${key(22)}\nAddress = 10.66.0.3/32\n\n[Peer]\nPublicKey = ${serverKey}\nEndpoint = 198.51.100.1:51820\nAllowedIPs = 0.0.0.0/0\n`;
const mime = { "index.html": "text/html", "app.js": "text/javascript", "chart.js": "text/javascript", "app.css": "text/css" };
const peers = () => [{ public_key: key(0), name: "Аня", revoked: false, can_share: true }, { public_key: key(10), name: "Старый доступ", revoked: true, can_share: false }];
function status() {
  return { version: "v9.9.9", update: { running: false, result: null }, vpn_enabled: true, tunnel_active: true, interface: "wg0", active_server_key: serverKey, servers: [{ name: "Test VPS", endpoint: "198.51.100.1:51820", public_key: serverKey, managed: true }, { name: "Imported VPN", endpoint: "198.51.100.2:51820", public_key: otherKey, managed: false }], peer: { public_key: serverKey, endpoint: "198.51.100.1:51820", allowed_ips: ["0.0.0.0/0"], latest_handshake: 1, handshake_age_seconds: 1, rx_bytes: 1024, tx_bytes: 512, persistent_keepalive: null }, stats: { rx_bps: 1200, tx_bps: 800 }, history: [{ timestamp: 1, rx_bps: 1200, tx_bps: 800 }], routing: { config: { mode: "rules", rule_order: null, domain_rules: [], ip_rules: [], default_target: "direct" }, dns_active: true, fake_ips: 0, geosite_loaded: true, geoip_loaded: true, dataplane_active: true, degraded: false } };
}
const json = (data, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(data) });
const managementPath = `/?server=${encodeURIComponent(serverKey)}#/server-management`;
const adminPassword = "пароль12";
const stageLabels = {
  waiting: "Ожидаем начало настройки", host_key: "Проверяем ключ SSH", connect: "Подключаемся к VPS",
  inspect: "Проверяем сервер", install: "Устанавливаем Gofro VPN", authorize: "Настраиваем доступ роутера",
  profile: "Создаём профиль VPN", save: "Сохраняем сервер на роутере",
};
let passed = 0;
let screenshots = 0;

async function open(browser, path = "/#/", width = 1440, setup = false, theme = "dark") {
  const state = { auth: setup ? { state: "setup", csrf_token: "setup-csrf", setup_method: "code", setup_window_seconds: 900 } : { state: "authenticated", csrf_token: "csrf" }, step: setup ? "admin" : "complete", peers: peers(), status: status(), fail: null, requests: [], expectedErrors: 0, nextPeer: 1, adminPassword };
  const context = await browser.newContext({ viewport: { width, height: width < 920 ? 844 : 1000 }, colorScheme: theme, reducedMotion: "no-preference", serviceWorkers: "block" });
  const errors = [];
  const consoleErrors = [];
  await context.exposeBinding("mockBootstrapRequest", (_source, request) => {
    try {
      assert.equal(request.method, "POST");
      assert.equal(request.headers["x-csrf-token"], state.auth.csrf_token);
      assert.equal(request.headers.accept, "application/x-ndjson");
      assert.deepEqual(request.body, { name: "Test VPS", host: "1.1.1.1", port: 2222, password: "root-test-password" });
      assert.equal(Object.hasOwn(request.body, "host_key"), false);
      state.requests.push({ endpoint: "POST /api/servers/bootstrap", body: request.body });
    } catch (error) { errors.push(error.message); throw error; }
  });
  // Playwright route.fulfill buffers bodies. Mock only fetch's transport for this
  // endpoint so the built app consumes a real, incrementally delivered byte stream.
  await context.addInitScript(() => {
    const fetch = window.fetch.bind(window);
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === "string" ? input : input.url, location.href);
      if (url.origin !== location.origin || url.pathname !== "/api/servers/bootstrap") return fetch(input, options);
      await window.mockBootstrapRequest({ method: options.method, headers: Object.fromEntries(new Headers(options.headers)), body: JSON.parse(options.body) });
      options.signal.throwIfAborted();
      window.mockBootstrapSignal = options.signal;
      return new Response(new ReadableStream({ start(controller) {
        window.mockBootstrapController = controller;
        options.signal.addEventListener("abort", () => controller.error(options.signal.reason), { once: true });
      } }), { headers: { "Content-Type": "application/x-ndjson" } });
    };
  });
  await context.route("**/*", async route => {
    try {
      const req = route.request();
      const url = new URL(req.url());
      assert.equal(url.origin, origin, `unexpected origin: ${url.origin}`);
      if (!url.pathname.startsWith("/api/")) {
        const name = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
        assert.ok(Object.hasOwn(mime, name), `unexpected asset ${name}`);
        return await route.fulfill({ contentType: mime[name], body: await readFile(new URL(name, assets)) });
      }
      const method = req.method();
      const endpoint = `${method} ${url.pathname}`;
      const body = method === "GET" ? null : req.postDataJSON();
      state.requests.push({ endpoint, body });
      assert.notEqual(url.pathname, "/api/servers/probe", "bootstrap must not probe or ask for a fingerprint");
      if (method !== "GET") assert.equal(req.headers()["x-csrf-token"], state.auth.csrf_token);
      if (state.fail?.endpoint === endpoint) {
        const failure = state.fail;
        state.fail = null;
        state.expectedErrors++;
        return await route.fulfill(json({ error: failure.error ?? "mock_failure" }, failure.status ?? 500));
      }
      const onboarding = () => ({ step: state.step, networks: [], setup_window_seconds: state.step === "admin" ? 900 : null, error: null });
      const managed = () => ({ version: "v1", peers: state.peers });
      if (endpoint === "GET /api/auth/status") return await route.fulfill(json(state.auth));
      if (endpoint === "POST /api/auth/setup") {
        assert.deepEqual(body, { password: state.adminPassword, setup_code: "mock-setup-code" });
        assert.ok(Array.from(body.password).length >= 8 && Buffer.byteLength(body.password) <= 128);
        assert.equal(state.step, "admin");
        state.auth = { state: "authenticated", csrf_token: "new-csrf" }; state.step = "server";
        return await route.fulfill(json(state.auth));
      }
      if (endpoint === "POST /api/auth/logout") {
        state.auth = { state: "login", csrf_token: "logout-csrf" };
        return await route.fulfill(json(state.auth));
      }
      if (endpoint === "GET /api/onboarding") return await route.fulfill(json(onboarding()));
      if (endpoint === "POST /api/onboarding/complete") {
        assert.equal(state.step, "server"); state.step = "complete";
        return await route.fulfill(json(onboarding()));
      }
      if (endpoint === "GET /api/status") return await route.fulfill(json(state.status));
      if (endpoint === "POST /api/mode") {
        assert.equal(typeof body.vpn_enabled, "boolean");
        state.status.vpn_enabled = body.vpn_enabled; state.status.tunnel_active = body.vpn_enabled;
        return await route.fulfill(json(state.status));
      }
      if (endpoint === "POST /api/servers/select") {
        assert.ok(state.status.servers.some(server => server.public_key === body.public_key));
        state.status.active_server_key = body.public_key; state.status.peer.public_key = body.public_key;
        return await route.fulfill(json(state.status));
      }
      if (endpoint === "POST /api/servers/import") {
        assert.deepEqual(body, { name: "My VPN", profile: profile.trim() });
        state.status.servers = [{ name: body.name, endpoint: "198.51.100.1:51820", public_key: serverKey, managed: false }];
        return await route.fulfill(json(state.status));
      }
      if (url.pathname.startsWith("/api/servers/")) {
        assert.equal(body.public_key, serverKey);
        if (endpoint === "POST /api/servers/management" || endpoint === "POST /api/servers/restart") return await route.fulfill(json(managed()));
        if (endpoint === "POST /api/servers/check" || endpoint === "POST /api/servers/update-managed") return await route.fulfill(json({ version: "v1", update_available: false }));
        if (endpoint === "POST /api/servers/friends") {
          assert.ok(body.name.trim());
          assert.ok(!state.peers.some(peer => !peer.revoked && peer.name.toLowerCase() === body.name.toLowerCase()));
          state.peers.push({ public_key: key(state.nextPeer++), name: body.name, revoked: false, can_share: true });
          return await route.fulfill(json(managed()));
        }
        const peer = state.peers.find(peer => peer.public_key === body.peer_key && !peer.revoked);
        assert.ok(peer, "must address a live friend, not the router or a revoked peer");
        if (endpoint === "PUT /api/servers/friends") { peer.name = body.name; return await route.fulfill(json(managed())); }
        if (endpoint === "DELETE /api/servers/friends") { peer.revoked = true; peer.can_share = false; return await route.fulfill(json(managed())); }
        if (endpoint === "POST /api/servers/friends/profile") { assert.ok(peer.can_share); return await route.fulfill(json({ profile })); }
      }
      if (endpoint === "POST /api/update") { state.status.update.result = "current"; return await route.fulfill(json(state.status)); }
      if (endpoint === "POST /api/routing") { state.status.routing.config = body; return await route.fulfill(json(state.status)); }
      if (endpoint === "POST /api/routing/test") return await route.fulfill(json({ value: body.value, target: "direct", matched_rule: null, scope: "domain_preview" }));
      throw new Error(`unexpected request ${endpoint}`);
    } catch (error) { errors.push(error.message); await route.abort(); }
  });
  const page = await context.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", error => errors.push(error.message));
  page.on("console", message => { if (message.type() === "error") consoleErrors.push(message.text()); });
  await page.goto(`${origin}${path}`);
  if (setup) await page.locator('input[autocomplete="one-time-code"]').waitFor();
  else await page.locator(".app-main .connection, .app-main .panel").first().waitFor();
  return { context, page, state, errors, consoleErrors };
}
async function close(test, name) {
  assert.equal(test.state.requests.some(req => req.endpoint.includes("/api/servers/probe")), false);
  for (const req of test.state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap")) assert.equal(Object.hasOwn(req.body, "host_key"), false);
  assert.deepEqual(test.errors, []);
  assert.ok(test.consoleErrors.every(message => /Failed to load resource: the server responded with a status of/.test(message)), test.consoleErrors.join("\n"));
  assert.equal(test.consoleErrors.length, test.state.expectedErrors, "unexpected browser resource errors");
  await test.context.close();
  console.log(`PASS ${++passed}: ${name}`);
}
async function screenshot(page, name) {
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "horizontal page overflow");
  assert.deepEqual(await page.locator("button:visible, input:visible, textarea:visible, select:visible, .panel:visible").evaluateAll(elements => elements.filter(el => { const r = el.getBoundingClientRect(); return r.left < -1 || r.right > innerWidth + 1; }).map(el => el.outerHTML.slice(0, 150))), [], "clipped controls");
  if (process.env.SCREENSHOT_DIR) {
    await mkdir(process.env.SCREENSHOT_DIR, { recursive: true });
    await page.screenshot({ path: `${process.env.SCREENSHOT_DIR}/${name}.png`, fullPage: true }); screenshots++;
  }
}
async function friends(page, names) {
  await page.waitForFunction(expected => JSON.stringify([...document.querySelectorAll(".peer-row h3")].map(el => el.textContent)) === JSON.stringify(expected), names);
  assert.equal(await page.locator(".management .count").textContent(), String(names.length));
}

async function setupAdmin(test, password = adminPassword) {
  test.state.adminPassword = password;
  await test.page.locator('input[autocomplete="one-time-code"]').fill("mock-setup-code");
  for (const input of await test.page.locator('input[autocomplete="new-password"]').all()) await input.fill(password);
  await test.page.getByRole("button", { name: "Продолжить" }).click();
  await test.page.getByRole("heading", { name: "Подключите VPN" }).waitFor();
}

async function startVps(test, onboarding = false) {
  const { page } = test;
  if (onboarding) await page.getByRole("button", { name: "Подключить свой сервер" }).click();
  else {
    await page.getByRole("button", { name: "Добавить", exact: true }).click();
    await page.getByRole("button", { name: /Свой сервер/ }).click();
  }
  await page.getByLabel("Название", { exact: true }).fill("Test VPS");
  await page.getByLabel("Публичный IPv4-адрес VPS").fill("1.1.1.1");
  await page.getByLabel("Порт SSH").fill("2222");
  await page.getByLabel("Пароль root", { exact: true }).fill("root-test-password");
  await page.getByRole("button", { name: "Настроить сервер", exact: true }).click();
  await page.waitForFunction(() => !!window.mockBootstrapController);
  assert.equal(test.state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap").length, 1, "one submit starts bootstrap immediately");
  await page.getByRole("heading", { name: "Настраиваем ваш сервер" }).waitFor();
  assert.equal(await page.locator(".bootstrap-stages li").count(), 1);
  await page.getByRole("dialog").getByRole("status").getByText(stageLabels.waiting).waitFor();
  await page.waitForTimeout(250);
  assert.equal(await page.locator(".bootstrap-stages li").count(), 1, "no invented progress while waiting for bytes");
  assert.ok(await page.getByRole("button", { name: "Закрыть", exact: true }).isDisabled());
  assert.equal(await page.getByRole("dialog").locator("form").count(), 0);
  assert.equal(await page.getByRole("dialog").getByText(/SHA256|отпечаток|подтвердить ключ/i).count(), 0);
  await page.keyboard.press("Enter");
  await page.keyboard.press("Escape");
  assert.ok(await page.getByRole("dialog").isVisible());
  assert.equal(test.state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap").length, 1);
  const spinner = page.locator(".bootstrap-symbol svg");
  assert.equal(await spinner.evaluate(el => getComputedStyle(el).animationName), "spin");
  await page.emulateMedia({ reducedMotion: "reduce" });
  assert.equal(await spinner.evaluate(el => getComputedStyle(el).animationName), "none");
  await page.emulateMedia({ reducedMotion: "no-preference" });
}

async function streamEvent(page, event, end = false) {
  // Split JSON and a multibyte codepoint, with real delays between byte chunks.
  const bytes = Buffer.from(`${JSON.stringify(event)}\n`);
  const unicode = bytes.findIndex(byte => byte >= 0xc0);
  const split = unicode < 0 ? Math.floor(bytes.length / 2) : unicode + 1;
  for (const chunk of [bytes.subarray(0, split), bytes.subarray(split)]) {
    await page.waitForTimeout(40);
    await page.evaluate(data => window.mockBootstrapController.enqueue(new Uint8Array(data)), [...chunk]);
  }
  if (end) await page.evaluate(() => window.mockBootstrapController.close());
}

async function streamStages(page, stages) {
  const seen = ["waiting"];
  for (const stage of stages) {
    await streamEvent(page, { type: "stage", stage });
    if (!seen.includes(stage)) seen.push(stage);
    await page.waitForFunction(label => document.querySelector(".stage-current")?.textContent.includes(label), stageLabels[seen.at(-1)]);
    assert.equal(await page.locator(".bootstrap-stages li").count(), seen.length, "duplicate stages are not rendered");
    assert.equal(await page.locator(".stage-done").count(), seen.length - 1);
    assert.equal(await page.locator(".stage-current").count(), 1);
    assert.equal(await page.getByRole("heading", { name: "Сервер добавлен", exact: true }).count(), 0);
  }
}

for (const name of ["app.js", "app.css", "chart.js"]) {
  const bytes = await readFile(new URL(name, assets));
  const hash = createHash("sha256").update(bytes).digest("hex");
  assert.equal(hash, await readFile(new URL(`${name}.sha256`, assets), "utf8"));
  assert.deepEqual(gunzipSync(await readFile(new URL(`${name}.gz`, assets))), bytes);
  const reference = await readFile(new URL(name === "chart.js" ? "app.js" : "index.html", assets), "utf8");
  assert.ok(reference.includes(`${name}?v=${hash}`), "generated cache-busting reference matches hash");
  console.log(`ASSET ${name} sha256=${hash}; gzip and reference verified`);
}
const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH ?? (process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : undefined) });
try {
  for (const width of [1440, 390, 320]) {
    const test = await open(browser, "/#/", width);
    const { page } = test;
    for (const [label, heading] of [["Обзор", "Трафик VPN"], ["Серверы", "Ваши серверы"], ["Правила", "Как использовать VPN"], ["Панель", "Обновления Gofro"]]) {
      const nav = page.locator(width < 920 ? ".mobile-nav" : ".nav-list");
      assert.deepEqual(await nav.locator("a").allTextContents(), ["Обзор", "Серверы", "Правила", "Панель"]);
      await nav.getByRole("link", { name: label, exact: true }).click();
      await page.getByRole("heading", { name: heading, exact: false }).waitFor();
      assert.equal(await page.locator(".side-bottom").count(), 0);
      assert.equal(await page.getByText(/Wi-Fi|Устройства|Диагностика|Перезагрузить|DHCP|Температура|Домашний роутер/).count(), 0);
      assert.equal(await page.locator(".sidebar").getByText(/192\.168\.|router\.gofro/).count(), 0);
      await screenshot(page, `navigation-${width}-${label}`);
    }
    await close(test, `navigation and layout ${width}px`);
  }

  for (const width of [1440, 390]) {
    const test = await open(browser, "/#/", width, true);
    const { page, state } = test;
    state.status.servers = [];
    assert.match(await page.locator(".access-note").innerText(), /gofro setup.*15 минут/);
    assert.doesNotMatch(await page.locator(".access-note").innerText(), /15:01/);
    assert.equal(await page.locator(".setup-progress li").count(), 2);
    await screenshot(page, `setup-code-${width}`);
    await page.locator('input[autocomplete="one-time-code"]').fill("wrong-code");
    await page.locator('input[autocomplete="new-password"]').nth(0).fill(adminPassword);
    await page.locator('input[autocomplete="new-password"]').nth(1).fill(adminPassword);
    state.fail = { endpoint: "POST /api/auth/setup", status: 403, error: "invalid_setup_code" };
    await page.getByRole("button", { name: "Продолжить" }).click();
    await page.getByText("Неверный одноразовый код установки.").waitFor();
    await page.locator('input[autocomplete="one-time-code"]').fill("mock-setup-code");
    await page.getByRole("button", { name: "Продолжить" }).click();
    await page.getByRole("heading", { name: "Подключите VPN" }).waitFor();
    assert.equal(state.step, "server");
    await page.getByRole("button", { name: "Импортировать настройки" }).click();
    await page.getByLabel("Название", { exact: true }).fill("My VPN");
    await page.getByLabel("Или вставьте настройки").fill(profile);
    await page.getByRole("button", { name: "Импортировать", exact: true }).click();
    await page.getByRole("dialog").waitFor({ state: "hidden" });
    await screenshot(page, `setup-server-${width}`);
    state.fail = { endpoint: "POST /api/onboarding/complete" };
    await page.getByRole("button", { name: "Завершить настройку" }).click();
    await page.getByRole("alert").getByText("mock_failure", { exact: true }).waitFor();
    await page.getByRole("button", { name: "Завершить настройку" }).click();
    await page.getByRole("heading", { name: "Трафик VPN" }).waitFor();
    await page.reload();
    await page.getByRole("heading", { name: "Трафик VPN" }).waitFor();
    assert.equal(state.step, "complete");
    await close(test, `code -> admin -> import VPN -> complete -> refresh ${width}px`);
  }

  let test = await open(browser, "/#/", 390, true);
  test.state.auth.setup_window_seconds = 1;
  await test.page.reload();
  await test.page.getByText(/Время настройки истекло/).waitFor();
  assert.ok(await test.page.locator('input[autocomplete="one-time-code"]').isDisabled());
  await screenshot(test.page, "setup-expired");
  test.state.auth.setup_window_seconds = 900;
  await test.page.getByRole("button", { name: "Повторить проверку" }).click();
  await test.page.locator('input[autocomplete="one-time-code"]').waitFor();
  assert.equal(await test.page.locator('input[autocomplete="one-time-code"]').isDisabled(), false);
  await close(test, "expired setup window and renewed console session");

  for (const width of [1440, 390]) {
    test = await open(browser, managementPath, width);
    const { page, state } = test;
    await friends(page, ["Аня"]);
    await page.getByRole("button", { name: "Создать доступ", exact: true }).click();
    await page.getByLabel("Имя друга").fill("аня");
    await page.getByRole("dialog").getByRole("button", { name: "Создать доступ" }).click();
    await page.getByText("Активный доступ с таким именем уже существует.").waitFor();
    assert.equal(state.requests.filter(req => req.endpoint === "POST /api/servers/friends").length, 0);
    await page.getByLabel("Имя друга").fill("Борис");
    state.fail = { endpoint: "POST /api/servers/friends" };
    await page.getByRole("dialog").getByRole("button", { name: "Создать доступ" }).click();
    await page.getByRole("dialog").getByText("mock_failure", { exact: true }).waitFor();
    await page.getByRole("dialog").getByRole("button", { name: "Создать доступ" }).click();
    await friends(page, ["Аня", "Борис"]);
    await page.getByRole("button", { name: "Изменить доступ Борис" }).click();
    await page.getByLabel("Имя друга").fill("Боря");
    await page.getByRole("button", { name: "Сохранить", exact: true }).click();
    await friends(page, ["Аня", "Боря"]);
    const friend = page.locator(".peer-row").filter({ has: page.getByRole("heading", { name: "Боря", exact: true }) });
    for (let attempt = 0; attempt < 2; attempt++) {
      if (attempt === 0) state.fail = { endpoint: "POST /api/servers/friends/profile" };
      await friend.getByRole("button", { name: "Поделиться" }).click();
      if (attempt === 0) {
        await page.getByRole("dialog").getByText("mock_failure", { exact: true }).waitFor();
        await page.getByRole("dialog").getByRole("button", { name: "Повторить" }).click();
      }
      await page.getByLabel("Конфигурация", { exact: true }).waitFor();
      assert.equal(await page.getByLabel("Конфигурация", { exact: true }).inputValue(), profile);
      const downloadPromise = page.waitForEvent("download");
      await page.getByRole("button", { name: "Скачать wg.conf" }).click();
      const download = await downloadPromise;
      assert.equal(download.suggestedFilename(), "wg.conf");
      assert.equal(await readFile(await download.path(), "utf8"), profile);
      await page.getByRole("button", { name: "Копировать текст" }).click();
      await page.getByText(/Конфигурация скопирована\.|Выделите конфигурацию и скопируйте вручную\./).waitFor();
      await screenshot(page, `friend-export-${width}-${attempt}`);
      await page.keyboard.press("Escape");
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      assert.equal(await page.locator("#peer-profile").count(), 0);
      assert.equal(await page.evaluate(() => JSON.stringify(localStorage) + JSON.stringify(sessionStorage)).then(text => text.includes("PrivateKey")), false);
      await page.reload(); await friends(page, ["Аня", "Боря"]);
    }
    await page.getByRole("button", { name: "Изменить доступ Боря" }).click();
    await page.getByRole("button", { name: "Отозвать", exact: true }).click();
    state.fail = { endpoint: "DELETE /api/servers/friends" };
    await page.getByRole("dialog", { name: "Отозвать доступ?" }).getByRole("button", { name: "Отозвать", exact: true }).click();
    await page.getByRole("dialog").getByText("mock_failure", { exact: true }).waitFor();
    await friends(page, ["Аня", "Боря"]);
    assert.equal(state.peers.find(peer => peer.name === "Боря").revoked, false);
    await page.reload(); await friends(page, ["Аня", "Боря"]);
    await page.getByRole("button", { name: "Изменить доступ Боря" }).click();
    await page.getByRole("button", { name: "Отозвать", exact: true }).click();
    await page.getByRole("dialog", { name: "Отозвать доступ?" }).getByRole("button", { name: "Отозвать", exact: true }).click();
    await friends(page, ["Аня"]);
    await page.reload(); await friends(page, ["Аня"]);
    assert.ok(state.peers.find(peer => peer.name === "Боря").revoked);
    assert.equal(state.peers.length, 3, "revoked historical rows remain in backend responses");
    await page.getByText("Обслуживание", { exact: true }).click();
    await page.getByRole("button", { name: "Перезапустить VPN", exact: true }).click();
    await page.getByRole("button", { name: "Перезапустить", exact: true }).click();
    await page.getByText(/VPN перезапущен: сервер ответил/).waitFor();
    await friends(page, ["Аня"]);
    assert.equal(state.status.vpn_enabled, true);
    await screenshot(page, `friends-${width}`);
    state.fail = { endpoint: "POST /api/servers/management" };
    await page.reload();
    await page.getByRole("heading", { name: "Доступы недоступны" }).waitFor();
    await page.getByRole("button", { name: "Повторить", exact: true }).click();
    await friends(page, ["Аня"]);
    await close(test, `friends create/rename/export/re-export/revoke/retry and VPS restart ${width}px`);
  }

  test = await open(browser);
  await test.page.getByRole("button", { name: "Отключить VPN", exact: true }).click();
  await test.page.getByRole("button", { name: "Отключить", exact: true }).click();
  await test.page.getByRole("heading", { name: "VPN отключён", exact: true }).waitFor();
  await test.page.getByRole("button", { name: "Выбрать VPN-сервер" }).click();
  await test.page.getByRole("button", { name: /Imported VPN/ }).click();
  await test.page.getByRole("button", { name: "Подключить", exact: true }).click();
  await test.page.getByRole("heading", { name: "VPN подключён", exact: true }).waitFor();
  assert.equal(test.state.status.active_server_key, otherKey);
  test.state.fail = { endpoint: "GET /api/status" };
  await test.page.getByText(/Нет свежих данных/).waitFor();
  await test.page.getByText(/Нет свежих данных/).waitFor({ state: "hidden" });
  await screenshot(test.page, "vpn-switched");
  await close(test, "VPN toggle/select and poll error recovery");

  test = await open(browser, "/#/routing", 390);
  await test.page.getByRole("button", { name: "Весь интернет", exact: true }).click();
  await test.page.getByText("Правила сохранены, но сейчас не применяются.").waitFor();
  assert.equal(test.state.status.routing.config.mode, "all");
  await test.page.getByRole("button", { name: "По правилам", exact: true }).click();
  await test.page.getByRole("button", { name: "Добавить", exact: true }).click();
  await test.page.getByLabel("Название", { exact: true }).fill("Example VPN");
  await test.page.getByLabel("Значение", { exact: false }).fill("example.com");
  await test.page.getByRole("button", { name: "Сохранить", exact: true }).click();
  await test.page.getByRole("dialog").waitFor({ state: "hidden" });
  await test.page.reload();
  await test.page.getByRole("heading", { name: "Example VPN", exact: true }).waitFor();
  assert.equal(test.state.status.routing.config.domain_rules[0].matcher.value, "example.com");
  await test.page.getByLabel("Адрес сайта или IP").fill("example.org");
  await test.page.getByRole("button", { name: "Проверить", exact: true }).click();
  await test.page.getByText("example.org → Без VPN", { exact: true }).waitFor();
  await screenshot(test.page, "rules-mobile");
  await close(test, "rules mode/save/refresh and route preview");

  test = await open(browser);
  test.state.fail = { endpoint: "GET /api/status", status: 401, error: "session_expired" };
  await test.page.getByRole("heading", { name: "Вход в панель" }).waitFor();
  assert.equal(await test.page.locator(".app-main").count(), 0);
  assert.equal(await test.page.getByRole("button", { name: "Первый запуск" }).count(), 0);
  await close(test, "session expiry hides authenticated UI");

  for (const result of [null, "updated", "current", "failed", "running"]) {
    test = await open(browser, "/#/system");
    test.state.status.update = { running: result === "running", result: result === "running" ? null : result };
    await test.page.reload();
    await test.page.locator(".version").waitFor();
    assert.equal((await test.page.locator("body").innerText()).split("v9.9.9").length - 1, 1);
    if (result === "current") await test.page.getByText("Обновлений нет.", { exact: true }).waitFor();
    if (result === null) {
      await test.page.getByRole("button", { name: "Проверить обновления" }).click();
      await test.page.getByRole("button", { name: "Проверить и обновить" }).click();
      await test.page.getByText("Обновлений нет.", { exact: true }).waitFor();
      await test.page.getByRole("button", { name: "Светлая", exact: true }).click();
      assert.equal(await test.page.locator("html").getAttribute("data-theme"), "light");
      await test.page.reload();
      await test.page.locator(".version").waitFor();
      assert.equal(await test.page.locator("html").getAttribute("data-theme"), "light");
      await screenshot(test.page, "panel-current-light");
      await test.page.getByRole("button", { name: "Выйти", exact: true }).click();
      await test.page.getByRole("heading", { name: "Вход в панель" }).waitFor();
      assert.equal(await test.page.getByText("Первый запуск", { exact: true }).count(), 0);
    }
    await close(test, `panel update ${result}, single version${result === null ? ", theme persistence, logout" : ""}`);
  }
  for (const width of [1440, 390, 320]) for (const theme of ["dark", "light"]) {
    test = await open(browser, "/#/", width, true, theme);
    const { page, state } = test;
    state.status.servers = [];
    await page.getByText("Минимум 8 символов, максимум 128 байт.", { exact: true }).waitFor();
    assert.equal(await page.locator('input[autocomplete="new-password"]').first().getAttribute("aria-describedby"), "password-requirement");
    await screenshot(page, `setup-code-${width}-${theme}`);
    await page.locator('input[autocomplete="one-time-code"]').fill("mock-setup-code");
    for (const password of ["1234567", "😀".repeat(7), "я".repeat(64) + "a"]) {
      for (const input of await page.locator('input[autocomplete="new-password"]').all()) await input.fill(password);
      await page.getByRole("button", { name: "Продолжить" }).click();
      if (password.includes("😀")) await page.getByRole("alert").getByText(/не менее 8 символов/).waitFor();
      if (Buffer.byteLength(password) > 128) await page.getByRole("alert").getByText(/Максимум 128 байт/).waitFor();
      assert.equal(state.requests.filter(req => req.endpoint === "POST /api/auth/setup").length, 0, "invalid password is refused by frontend");
    }
    await screenshot(page, `password-byte-limit-${width}-${theme}`);
    await setupAdmin(test, theme === "light" ? "😀".repeat(32) : adminPassword);
    await screenshot(page, `onboarding-${width}-${theme}`);
    await startVps(test, true);
    await screenshot(page, `vps-waiting-${width}-${theme}`);
    await streamStages(page, ["waiting", "host_key", "host_key", "connect", "inspect", "install", "authorize", "profile", "save"]);
    await screenshot(page, `vps-running-${width}-${theme}`);
    const completed = status();
    completed.servers = [completed.servers[0]];
    completed.vpn_enabled = false; completed.tunnel_active = false;
    await streamEvent(page, { type: "complete", status: completed }, true);
    await page.getByRole("heading", { name: "Сервер добавлен", exact: true }).waitFor();
    assert.equal(await page.locator(".stage-done").count(), 8);
    assert.equal(await page.locator(".stage-current").count(), 0);
    assert.equal(await page.getByLabel("Пароль root", { exact: true }).count(), 0);
    assert.equal(await page.locator(".bootstrap-progress svg").evaluateAll(elements => elements.some(el => getComputedStyle(el).animationName === "spin")), false);
    await screenshot(page, `vps-complete-${width}-${theme}`);
    await page.getByRole("button", { name: "Готово", exact: true }).click();
    await page.getByRole("button", { name: "Завершить настройку" }).waitFor();
    state.status = completed;
    await page.getByRole("button", { name: "Завершить настройку" }).click();
    await page.getByRole("heading", { name: "VPN отключён", exact: true }).waitFor();
    await page.reload();
    await page.getByRole("heading", { name: "VPN отключён", exact: true }).waitFor();
    assert.equal(state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap").length, 1);
    await close(test, `7 Unicode chars refused, 8 accepted / UTF-8 129 refused, 128 accepted; code -> streaming VPS -> final status ${width}px ${theme}`);
  }

  test = await open(browser, "/#/servers", 390);
  const history = structuredClone(test.state.peers);
  await startVps(test);
  await streamStages(test.page, ["waiting", "host_key", "connect", "inspect", "authorize", "profile", "save"]);
  assert.equal(await test.page.locator(".bootstrap-stages").getByText(stageLabels.install).count(), 0, "compatible re-add skips installation");
  const readded = structuredClone(test.state.status);
  readded.vpn_enabled = false; readded.tunnel_active = false;
  await streamEvent(test.page, { type: "complete", status: readded }, true);
  await test.page.getByRole("heading", { name: "Сервер добавлен", exact: true }).waitFor();
  await test.page.getByRole("button", { name: "Готово", exact: true }).click();
  await test.page.waitForFunction(() => [...document.querySelectorAll(".server-status")].every(el => el.textContent === "Отключён"));
  assert.equal(await test.page.locator(".server-detail-link").count(), 2, "re-add does not duplicate saved servers");
  test.state.status = readded;
  assert.deepEqual(test.state.peers, history);
  await test.page.goto(`${origin}${managementPath}`);
  await friends(test.page, ["Аня"]);
  await test.page.reload(); await friends(test.page, ["Аня"]);
  assert.deepEqual(test.state.peers, history, "active and revoked friend history survives re-add and refresh");
  await screenshot(test.page, "vps-readd-preserved-friends");
  await close(test, "compatible VPS re-add: no install, no duplicate server, final status applied, friend history preserved");

  for (const failure of [...Object.keys(stageLabels), "eof", "malformed", "disconnect", "deadline"]) {
    test = await open(browser, "/#/servers", 320, false, "light");
    const { page, state } = test;
    const before = structuredClone(state.status);
    if (failure === "deadline") await page.clock.install();
    await startVps(test);
    await streamStages(page, Object.hasOwn(stageLabels, failure) ? Object.keys(stageLabels).slice(0, Object.keys(stageLabels).indexOf(failure)) : ["waiting", "host_key"]);
    if (Object.hasOwn(stageLabels, failure)) {
      await streamEvent(page, { type: "error", stage: failure, message: `Ошибка этапа ${failure}: тестовый отказ` }, true);
      await page.getByRole("dialog").getByRole("alert").getByText(`Ошибка этапа ${failure}: тестовый отказ`, { exact: true }).waitFor();
      await page.locator(".bootstrap-stages").getByText(stageLabels[failure], { exact: false }).waitFor();
      assert.match(await page.locator(".stage-current").innerText(), new RegExp(stageLabels[failure]));
    } else {
      if (failure === "malformed") await streamEvent(page, { type: "complete", status: { secret: "must-not-leak" } });
      else if (failure === "deadline") {
        assert.equal(await page.evaluate(() => window.mockBootstrapSignal.aborted), false);
        await page.clock.fastForward(30 * 60_000);
        assert.equal(await page.evaluate(() => window.mockBootstrapSignal.aborted), true);
        assert.equal(await page.evaluate(() => window.mockBootstrapSignal.reason.name), "AbortError");
      }
      else await page.evaluate(disconnect => {
        if (disconnect) window.mockBootstrapController.error(new TypeError("mock connection lost"));
        else window.mockBootstrapController.close();
      }, failure === "disconnect");
      await page.getByRole("dialog").getByRole("alert").getByText(/Результат настройки неизвестен/).waitFor();
      const guidance = await page.getByRole("dialog").getByRole("alert").innerText();
      assert.match(guidance, /Настройка на роутере и VPS может продолжаться/);
      assert.match(guidance, /Проверьте список серверов, состояние VPN на роутере и состояние VPS перед ручной повторной попыткой/);
      assert.match(guidance, /Запрос не будет повторён автоматически/);
      assert.match(await page.locator(".stage-current").innerText(), /Проверяем ключ SSH.*результат не подтверждён/s);
      assert.doesNotMatch(await page.getByRole("dialog").innerText(), /must-not-leak/);
    }
    await page.getByRole("heading", { name: "Настройка требует внимания" }).waitFor();
    await page.locator('.app-main > [role="alert"] strong').getByText("Операция требует внимания.", { exact: true }).waitFor();
    assert.doesNotMatch(await page.locator("body").innerText(), /Операция не выполнена/);
    assert.match(await page.locator(".bootstrap-caption").innerText(), /Изменения могли сохраниться\. Перед ручной повторной попыткой проверьте список серверов, состояние VPN на роутере и состояние VPS/);
    assert.equal(await page.getByLabel("Пароль root", { exact: true }).inputValue(), "");
    assert.equal(await page.getByLabel("Публичный IPv4-адрес VPS").inputValue(), "1.1.1.1");
    assert.equal(await page.getByLabel("Порт SSH").inputValue(), "2222");
    assert.equal(await page.getByRole("dialog").getByRole("button", { name: "Закрыть", exact: true }).isDisabled(), false);
    assert.equal(await page.getByRole("heading", { name: "Сервер добавлен", exact: true }).count(), 0);
    await screenshot(page, `vps-error-${failure}`);
    await page.getByRole("button", { name: "Повторить настройку", exact: true }).click();
    await page.waitForTimeout(1100);
    assert.equal(state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap").length, 1, "no auto-POST or passwordless retry after failure");
    assert.deepEqual(state.status, before);
    console.log(`PROOF ${failure}: ${await page.getByRole("dialog").getByRole("alert").innerText()}; banner=Операция требует внимания.; stage retained; password cleared; POST count=1`);
    await page.getByLabel("Пароль root", { exact: true }).fill("root-test-password");
    await page.evaluate(() => { window.mockBootstrapController = null; });
    await page.getByRole("button", { name: "Повторить настройку", exact: true }).click();
    await page.waitForFunction(() => !!window.mockBootstrapController);
    assert.equal(state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap").length, 2);
    assert.equal(await page.locator(".bootstrap-stages li").count(), 1, "explicit retry resets old stages");
    await streamStages(page, ["waiting", "host_key", "connect", "inspect", "authorize", "profile", "save"]);
    await streamEvent(page, { type: "complete", status: state.status }, true);
    await page.getByRole("heading", { name: "Сервер добавлен", exact: true }).waitFor();
    await page.getByRole("button", { name: "Готово", exact: true }).click();
    await page.reload();
    await page.getByRole("heading", { name: "Ваши серверы", exact: false }).waitFor();
    assert.equal(state.requests.filter(req => req.endpoint === "POST /api/servers/bootstrap").length, 2, "refresh never resubmits bootstrap");
    await close(test, `VPS ${failure}: visible failure, cleared password, no auto-retry, explicit retry succeeds`);
  }
  console.log(`PASS: ${passed} browser scenarios; ${screenshots} screenshots; 3 asset hashes/gzip verified. All requests mocked at ${origin}.`);
} finally { await browser.close(); }
