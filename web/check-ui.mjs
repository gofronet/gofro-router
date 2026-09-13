// Run after `bun run build`: PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs bun check-ui.mjs
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { gunzipSync } from "node:zlib";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const assets = new URL("../assets/", import.meta.url);
// Canonical origin, entirely fulfilled in-process. No device or network fallback.
const origin = "https://wifi.gofro.net";
const key = (n) => Buffer.from(String.fromCharCode(97 + n).repeat(32)).toString("base64");
const serverKey = key(20);
const otherKey = key(21);
const profile = `[Interface]\nPrivateKey = ${key(22)}\nAddress = 10.66.0.3/32\n\n[Peer]\nPublicKey = ${serverKey}\nEndpoint = 198.51.100.1:51820\nAllowedIPs = 0.0.0.0/0\n`;
const mime = { "index.html": "text/html", "app.js": "text/javascript", "chart.js": "text/javascript", "app.css": "text/css" };
const peers = () => [{ public_key: key(0), name: "Аня", revoked: false, can_share: true }, { public_key: key(10), name: "Старый доступ", revoked: true, can_share: false }];
const pickedMac = "02:ab:cd:ef:01:23";
const manualMac = "02:ab:cd:ef:01:24";
const offlineMac = "02:ab:cd:ef:01:25";
const inventory = () => ({ discovery: "complete", devices: [
  { mac: pickedMac, name: "Ноутбук", addresses: ["192.168.1.20", "2001:db8::20"] },
  { mac: "02:ab:cd:ef:01:26", name: "Телефон", addresses: ["192.168.1.21"] },
] });
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
  state.inventory = inventory();
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
  await context.exposeBinding("mockPasswordTimeout", (_source, request) => {
    try {
      assert.equal(request.method, "POST");
      assert.equal(request.credentials, "same-origin");
      assert.equal(request.headers["x-csrf-token"], state.auth.csrf_token);
      assert.deepEqual(request.body, state.passwordBody);
      state.requests.push({ endpoint: "POST /api/auth/password", body: request.body, csrf: request.headers["x-csrf-token"] });
      state.auth = { state: "authenticated", csrf_token: "uncertain-csrf" };
    } catch (error) { errors.push(error.message); throw error; }
  });
  // Playwright route.fulfill buffers bodies. Mock only fetch's transport for this
  // endpoint so the built app consumes a real, incrementally delivered byte stream.
  await context.addInitScript(() => {
    const fetch = window.fetch.bind(window);
    window.fetch = async (input, options) => {
      const url = new URL(typeof input === "string" ? input : input.url, location.href);
      if (url.origin === location.origin && url.pathname === "/api/auth/password" && window.passwordTimeout) {
        await window.mockPasswordTimeout({ method: options.method, credentials: options.credentials, headers: Object.fromEntries(new Headers(options.headers)), body: JSON.parse(options.body) });
        options.signal.throwIfAborted();
        window.passwordSignal = options.signal;
        return new Response(new ReadableStream({ start(controller) {
          options.signal.addEventListener("abort", () => controller.error(options.signal.reason), { once: true });
        } }));
      }
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
      state.requests.push({ endpoint, body, csrf: req.headers()["x-csrf-token"] });
      assert.notEqual(url.pathname, "/api/servers/probe", "bootstrap must not probe or ask for a fingerprint");
      if (method !== "GET") assert.equal(req.headers()["x-csrf-token"], state.auth.csrf_token);
      if (state.hold?.endpoint === endpoint) await state.hold.promise;
      if (endpoint === "POST /api/auth/password") {
        assert.deepEqual(body, state.passwordBody);
        assert.ok(Array.from(body.password).length >= 8 && Buffer.byteLength(body.password) <= 128);
        if (state.fail?.error === "password_change_uncertain") state.auth = { state: "authenticated", csrf_token: "uncertain-csrf" };
      }
      if (endpoint === "POST /api/device-exclusions") {
        assert.deepEqual(Object.keys(body).sort(), ["excluded", "mac"]);
        assert.match(body.mac, /^(?:[0-9a-f]{2}:){5}[0-9a-f]{2}$/);
        assert.equal(parseInt(body.mac.slice(0, 2), 16) & 1, 0);
        assert.notEqual(body.mac, "00:00:00:00:00:00");
        assert.equal(typeof body.excluded, "boolean");
        if (state.fail?.endpoint !== endpoint || state.fail.outcome === "committed") {
          const saved = state.status.device_exclusions ?? [];
          state.status.device_exclusions = body.excluded ? [...new Set([...saved, body.mac])] : saved.filter(mac => mac !== body.mac);
        }
      }
      if (state.fail?.endpoint === endpoint) {
        const failure = state.fail;
        if (!failure.keep) state.fail = null;
        state.expectedErrors++;
        return await route.fulfill(json({ error: failure.error ?? "mock_failure", ...(failure.outcome ? { outcome: failure.outcome } : {}) }, failure.status ?? 500));
      }
      const onboarding = () => ({ step: state.step, networks: [], setup_window_seconds: state.step === "admin" ? 900 : null, error: null });
      const managed = () => ({ version: "v1", peers: state.peers });
      if (endpoint === "GET /api/auth/status") return await route.fulfill(json(state.auth));
      if (endpoint === "POST /api/auth/password") {
        state.auth = { state: "authenticated", csrf_token: "password-rotated-csrf" };
        return await route.fulfill(json(state.auth));
      }
      if (endpoint === "POST /api/auth/login") {
        assert.deepEqual(body, { password: state.passwordBody?.password ?? state.adminPassword });
        state.auth = { state: "authenticated", csrf_token: "login-csrf" };
        return await route.fulfill(json(state.auth));
      }
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
      if (endpoint === "GET /api/lan-devices") return await route.fulfill(json(state.inventory));
      if (endpoint === "POST /api/device-exclusions") return await route.fulfill(json(state.status));
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
      if (endpoint === "POST /api/routing") { assert.equal(Object.hasOwn(body, "device_exclusions"), false); state.status.routing.config = body; return await route.fulfill(json(state.status)); }
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

const passwordPosts = state => state.requests.filter(req => req.endpoint === "POST /api/auth/password");
async function passwordOpen(page) {
  await page.getByRole("button", { name: "Сменить пароль", exact: true }).click();
  await page.getByRole("dialog", { name: "Сменить пароль", exact: true }).waitFor();
}
async function passwordFill(page, current, password, confirmation = password) {
  for (const [label, value] of [["Текущий пароль", current], ["Новый пароль", password], ["Повторите новый пароль", confirmation]]) await page.getByLabel(label, { exact: true }).fill(value);
}
const passwordSubmit = page => page.getByRole("dialog").getByRole("button", { name: "Сменить пароль", exact: true }).click();
async function passwordEmpty(page) {
  assert.deepEqual(await page.getByRole("dialog").locator("input").evaluateAll(inputs => inputs.map(input => input.value)), ["", "", ""]);
  assert.equal(await page.evaluate(() => JSON.stringify(localStorage) + JSON.stringify(sessionStorage)).then(text => /old-secret|new-secret/.test(text)), false);
}

async function passwordChecks(browser) {
  for (const width of [1440, 390]) {
    let test = await open(browser, "/#/system", width);
    let { page, state } = test;
    await passwordOpen(page);
    assert.deepEqual(await page.getByRole("dialog").locator("input").evaluateAll(inputs => inputs.map(input => [input.type, input.autocomplete, input.required])), [["password", "current-password", true], ["password", "new-password", true], ["password", "new-password", true]]);
    await screenshot(page, `password-open-${width}`);
    for (const [current, password, confirmation, message] of [
      ["", "new-secret", "new-secret", null],
      ["old-secret", "", "new-secret", null],
      ["old-secret", "new-secret", "", null],
      ["old-secret", "1234567", "1234567", /не менее 8 символов/],
      ["old-secret", "😀".repeat(7), "😀".repeat(7), /не менее 8 символов/],
      ["old-secret", "я".repeat(64) + "a", "я".repeat(64) + "a", /Максимум 128 байт/],
      ["old-secret", "😀".repeat(33), "😀".repeat(33), /Максимум 128 байт/],
      ["old-secret", "new-secret", "different", /Новые пароли не совпадают/],
    ]) {
      await passwordFill(page, current, password, confirmation);
      await passwordSubmit(page);
      if (message) await page.getByRole("dialog").getByRole("alert").getByText(message).waitFor();
      else assert.equal(await page.getByRole("dialog").locator("form").evaluate(form => form.checkValidity()), false);
      assert.equal(passwordPosts(state).length, 0);
    }
    await screenshot(page, `password-validation-${width}`);
    await page.getByRole("button", { name: "Отмена", exact: true }).click();
    await passwordOpen(page); await passwordEmpty(page);
    await passwordFill(page, "old-secret", "new-secret");
    await page.keyboard.press("Escape");
    await passwordOpen(page); await passwordEmpty(page);
    await passwordFill(page, "old-secret", "new-secret");
    await page.evaluate(() => { location.hash = "#/"; });
    await page.getByRole("heading", { name: "Трафик VPN" }).waitFor();
    await page.locator(width < 920 ? ".mobile-nav" : ".nav-list").getByRole("link", { name: "Панель", exact: true }).click();
    await passwordOpen(page); await passwordEmpty(page);
    assert.equal(passwordPosts(state).length, 0);
    await close(test, `password required/current/new/confirm, Unicode 7 and >128-byte refusal, mismatch no POST, cancel/Escape/unmount clear ${width}px`);

    test = await open(browser, "/#/system", width); ({ page, state } = test);
    await passwordOpen(page);
    state.passwordBody = { current_password: "wrong-current", password: "😀".repeat(8) };
    state.fail = { endpoint: "POST /api/auth/password", status: 400, error: "invalid_current_password" };
    await passwordFill(page, state.passwordBody.current_password, state.passwordBody.password);
    await passwordSubmit(page);
    await page.getByRole("dialog").getByText("Неверный текущий пароль.", { exact: true }).waitFor();
    await passwordEmpty(page);
    assert.equal(await page.locator(".app-main").count(), 1);
    assert.equal(state.auth.state, "authenticated");
    assert.equal(passwordPosts(state).length, 1);
    await screenshot(page, `password-wrong-current-${width}`);
    await close(test, `password Unicode 8 accepted, wrong-current 400 retains login and clears all fields ${width}px`);

    test = await open(browser, "/#/system", width); ({ page, state } = test);
    await passwordOpen(page);
    state.passwordBody = { current_password: "old-secret", password: "😀".repeat(32) };
    const held = Promise.withResolvers();
    state.hold = { endpoint: "POST /api/auth/password", promise: held.promise };
    const arrived = page.waitForRequest(req => req.url().endsWith("/api/auth/password"));
    await passwordFill(page, state.passwordBody.current_password, state.passwordBody.password);
    await passwordSubmit(page); await arrived;
    await page.getByRole("button", { name: "Сохраняем…", exact: true }).waitFor();
    assert.ok(await page.getByRole("button", { name: "Сохраняем…", exact: true }).isDisabled());
    assert.ok(await page.getByRole("button", { name: "Отмена", exact: true }).isDisabled());
    assert.ok(await page.getByRole("button", { name: "Выйти", exact: true }).isDisabled());
    assert.ok(await page.getByRole("button", { name: "Проверить обновления", exact: true }).isDisabled());
    assert.ok(await page.getByRole("dialog").locator("input").evaluateAll(inputs => inputs.every(input => input.disabled)));
    await page.getByRole("dialog").locator("form").evaluate(form => { form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    await page.keyboard.press("Enter"); await page.keyboard.press("Escape");
    assert.ok(await page.getByRole("dialog").isVisible());
    assert.equal(passwordPosts(state).length, 1);
    await screenshot(page, `password-busy-${width}`);
    held.resolve();
    await page.getByText("Пароль изменён. Остальные сеансы завершены.", { exact: true }).waitFor();
    assert.equal(await page.getByRole("dialog").count(), 0);
    assert.equal(await page.locator(".app-main").count(), 1);
    assert.equal(state.requests.filter(req => req.endpoint === "POST /api/auth/login" || req.endpoint === "POST /api/auth/logout").length, 0);
    await screenshot(page, `password-success-${width}`);
    await passwordOpen(page); await passwordEmpty(page);
    await page.getByRole("button", { name: "Отмена", exact: true }).click();
    await page.getByRole("button", { name: "Проверить обновления", exact: true }).click();
    await page.getByRole("button", { name: "Проверить и обновить", exact: true }).click();
    await page.getByText("Обновлений нет.", { exact: true }).waitFor();
    assert.equal(state.requests.find(req => req.endpoint === "POST /api/update").csrf, "password-rotated-csrf");
    await page.reload(); await page.locator(".version").waitFor();
    assert.equal(passwordPosts(state).length, 1);
    await close(test, `password exact POST, Unicode 128 bytes accepted, busy duplicate guard, login preserved, rotated CSRF next mutation ${width}px`);

    for (const failure of ["500", "timeout"]) {
      test = await open(browser, "/#/system", width); ({ page, state } = test);
      await passwordOpen(page);
      state.passwordBody = { current_password: "old-secret", password: "new-secret" };
      if (failure === "500") state.fail = { endpoint: "POST /api/auth/password", status: 500, error: "password_change_uncertain" };
      else await page.evaluate(() => { window.passwordTimeout = true; });
      await passwordFill(page, state.passwordBody.current_password, state.passwordBody.password);
      await passwordSubmit(page);
      if (failure === "timeout") {
        await page.waitForFunction(() => !!window.passwordSignal);
        assert.equal(await page.evaluate(() => window.passwordSignal.aborted), false);
        await page.waitForFunction(() => window.passwordSignal.aborted, null, { timeout: 35_000 });
        assert.equal(await page.evaluate(() => window.passwordSignal.aborted), true);
      }
      await page.getByRole("dialog").getByRole("alert").getByText(/Результат смены пароля неизвестен/).waitFor();
      const guidance = await page.getByRole("dialog").getByRole("alert").innerText();
      assert.match(guidance, /Запрос не повторён автоматически/);
      assert.match(guidance, /Обновите страницу.*попробуйте новый пароль/);
      await passwordEmpty(page);
      await screenshot(page, `password-${failure}-${width}`);
      await page.waitForTimeout(5500);
      assert.equal(passwordPosts(state).length, 1, "no automatic password replay after uncertainty");
      // A later expired poll preserves the guidance on the actual login screen.
      state.auth = { state: "login", csrf_token: "resync-csrf" };
      state.fail = { endpoint: "GET /api/status", status: 401, error: "session_expired" };
      await page.getByRole("heading", { name: "Вход в панель", exact: true, level: 1 }).waitFor();
      await page.getByRole("alert").getByText(/попробуйте новый пароль/).waitFor();
      await screenshot(page, `password-${failure}-login-${width}`);
      const resync = Promise.withResolvers();
      state.hold = { endpoint: "GET /api/auth/status", promise: resync.promise };
      await page.locator('input[autocomplete="current-password"]').fill("new-secret");
      try {
        await Promise.all([
          page.waitForRequest(req => req.url().endsWith("/api/auth/status")),
          page.getByRole("button", { name: "Войти", exact: true }).click({ timeout: 5000 }),
        ]);
      } catch (error) {
        console.error({ requests: state.requests.map(req => req.endpoint), errors: test.errors, body: await page.locator("body").innerText() });
        throw error;
      }
      await page.waitForTimeout(100);
      assert.equal(state.requests.filter(req => req.endpoint === "POST /api/auth/login").length, 0, "login waits for serialized status resync");
      resync.resolve();
      await page.locator(".app-main .panel").first().waitFor();
      assert.equal(state.requests.find(req => req.endpoint === "POST /api/auth/login").csrf, "resync-csrf");
      assert.deepEqual(state.requests.filter(req => req.endpoint.includes("/api/auth/")).map(req => req.endpoint), ["GET /api/auth/status", "POST /api/auth/password", "GET /api/auth/status", "POST /api/auth/login"]);
      assert.equal(passwordPosts(state).length, 1);
      await close(test, `password ${failure}: uncertain guidance survives login, secrets cleared, no replay, serialized GET resync before new-password login ${width}px`);
    }
  }
}

const devicePosts = state => state.requests.filter(req => req.endpoint === "POST /api/device-exclusions");
const devicePanel = page => page.locator('section[aria-labelledby="device-exclusions-title"]');
const deviceSwitch = (page, name = "Ноутбук") => devicePanel(page).getByRole("switch", { name: `Напрямую: ${name}`, exact: true });
async function checked(page, name, value) {
  await devicePanel(page).locator(`[role="switch"][aria-label="Напрямую: ${name}"][aria-checked="${value}"]`).waitFor();
}
async function addDevice(page, mac) {
  await page.getByLabel("MAC устройства", { exact: true }).fill(mac);
  await page.getByRole("button", { name: "Добавить напрямую", exact: true }).click();
}
async function refreshDevices(page) {
  await page.getByRole("button", { name: "Обновить список устройств", exact: true }).click();
  await page.getByRole("button", { name: "Обновить список устройств", exact: true }).waitFor();
}
async function deviceChecks(browser) {
  const baseline = passed;
  for (const width of [1440, 390, 320]) {
    const theme = width === 320 ? "light" : "dark";
    let test = await open(browser, "/#/routing", width, false, theme);
    let { page, state } = test;
    await checked(page, "Ноутбук", false);
    assert.equal(Object.hasOwn(state.status, "device_exclusions"), false, "legacy status omits exclusions");
    assert.equal(await devicePanel(page).locator(".count").innerText(), "0/256", "missing status field defaults to []");
    const copy = await devicePanel(page).innerText();
    assert.match(copy, /Включите переключатель, чтобы весь интернет устройства шёл напрямую/);
    await deviceSwitch(page).click();
    await checked(page, "Ноутбук", true);
    await checked(page, "Телефон", false);
    assert.deepEqual(state.status.device_exclusions, [pickedMac], "only selected MAC is full-direct");
    assert.deepEqual(devicePosts(state).map(req => req.body), [{ mac: pickedMac, excluded: true }]);
    state.inventory.devices[0].addresses = ["192.168.1.99", "2001:db8::99"];
    await refreshDevices(page);
    await devicePanel(page).getByText("192.168.1.99, 2001:db8::99", { exact: true }).waitFor();
    await checked(page, "Ноутбук", true);
    assert.equal(devicePosts(state).length, 1, "DHCP/IP refresh never rewrites the exclusion");
    await addDevice(page, ` ${manualMac.toUpperCase()} `);
    await checked(page, manualMac, true);
    assert.deepEqual(devicePosts(state).at(-1).body, { mac: manualMac, excluded: true });
    assert.equal(await page.getByLabel("MAC устройства", { exact: true }).inputValue(), "");
    await page.reload();
    await checked(page, manualMac, true);
    await checked(page, "Ноутбук", true);
    await screenshot(page, `devices-picked-manual-${width}-${theme}`);
    state.inventory.devices.shift();
    await refreshDevices(page);
    await checked(page, pickedMac, true);
    assert.deepEqual(state.status.device_exclusions, [pickedMac, manualMac], "disappearing discovered device remains saved");
    await close(test, `devices omitted status defaults [], discovered pick, only selected MAC, current IP, DHCP same MAC, uppercase manual canonical POST, reload ${width}px`);

    test = await open(browser, "/#/routing", width, false, theme); ({ page, state } = test);
    await checked(page, "Ноутбук", false);
    for (const invalid of ["not-a-mac", "02-ab-cd-ef-01-23", "02:gg:cd:ef:01:23", "ff:ff:ff:ff:ff:ff", "01:00:5e:00:00:01", "33:33:00:00:00:01", "00:00:00:00:00:00"]) {
      await addDevice(page, invalid);
      await devicePanel(page).getByRole("alert").waitFor();
      assert.equal(await page.getByLabel("MAC устройства", { exact: true }).getAttribute("aria-invalid"), "true");
      assert.equal(devicePosts(state).length, 0, `${invalid} refused before POST`);
    }
    await screenshot(page, `devices-invalid-${width}-${theme}`);
    await close(test, `devices invalid syntax, broadcast, IPv4/IPv6 multicast and zero refused without POST ${width}px`);

    test = await open(browser, "/#/routing", width, false, theme); ({ page, state } = test);
    state.status.device_exclusions = [pickedMac, offlineMac];
    await page.reload(); await checked(page, "Ноутбук", true); await checked(page, offlineMac, true);
    await page.clock.install();
    state.fail = { endpoint: "POST /api/device-exclusions", error: "remove_failed" };
    await deviceSwitch(page).click();
    await page.getByRole("alert").filter({ hasText: "remove_failed" }).waitFor();
    await checked(page, "Ноутбук", true);
    assert.ok(await deviceSwitch(page).isDisabled(), "failed mutation blocks stale follow-up");
    assert.deepEqual(state.status.device_exclusions, [pickedMac, offlineMac]);
    await screenshot(page, `devices-remove-failed-${width}-${theme}`);
    await page.clock.fastForward(5000);
    await page.waitForFunction(() => !document.querySelector('[aria-label="Напрямую: Ноутбук"]').disabled);
    assert.equal(devicePosts(state).length, 1);
    await deviceSwitch(page).click(); await checked(page, "Ноутбук", false);
    assert.deepEqual(state.status.device_exclusions, [offlineMac]);
    assert.deepEqual(devicePosts(state).map(req => req.body), [{ mac: pickedMac, excluded: false }, { mac: pickedMac, excluded: false }]);
    await close(test, `devices failed removal remains checked, stale blocked, refresh then explicit removal ${width}px`);

    test = await open(browser, "/#/routing", width, false, theme); ({ page, state } = test);
    await checked(page, "Ноутбук", false); await page.clock.install();
    state.fail = { endpoint: "POST /api/device-exclusions", outcome: "committed", error: "status_refresh_failed" };
    await deviceSwitch(page).click();
    await page.getByText(/Изменение выполнено, но состояние не удалось обновить/).waitFor();
    assert.deepEqual(state.status.device_exclusions, [pickedMac], "backend committed despite refresh error");
    assert.ok(await deviceSwitch(page).isDisabled());
    assert.ok(await page.getByRole("button", { name: "Добавить напрямую", exact: true }).isDisabled());
    await page.getByLabel("MAC устройства", { exact: true }).fill(manualMac);
    await devicePanel(page).locator("form").evaluate(form => form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
    state.fail = { endpoint: "GET /api/status", keep: true };
    await page.clock.fastForward(5000);
    await page.getByText(/Нет свежих данных/).waitFor();
    await page.clock.fastForward(5000);
    assert.equal(devicePosts(state).length, 1, "committed warning never replays POST");
    assert.ok(await deviceSwitch(page).isDisabled());
    await screenshot(page, `devices-committed-warning-${width}-${theme}`);
    state.fail = null;
    await page.getByRole("button", { name: "Обновить состояние перед изменением", exact: true }).click();
    await checked(page, "Ноутбук", true);
    await page.getByText(/Изменение выполнено, но состояние не удалось обновить/).waitFor({ state: "hidden" });
    assert.equal(devicePosts(state).length, 1);
    await close(test, `devices committed refresh warning, no replay, stale controls blocked through failed polls, separate GET recovers ${width}px`);

    for (const discovery of ["partial", "unavailable", "error"]) {
      test = await open(browser, "/#/routing", width, false, theme); ({ page, state } = test);
      state.status.device_exclusions = [offlineMac];
      state.inventory = { discovery: discovery === "partial" ? "partial" : "unavailable", devices: [] };
      if (discovery === "error") state.fail = { endpoint: "GET /api/lan-devices", error: "inventory_failed" };
      await page.reload(); await checked(page, offlineMac, true);
      await devicePanel(page).getByText(discovery === "partial" ? /Список устройств неполный/ : /Список устройств недоступен/).waitFor();
      if (discovery === "error") await devicePanel(page).getByText("inventory_failed", { exact: true }).waitFor();
      const before = structuredClone(state.status.routing.config);
      assert.equal(await page.getByRole("button", { name: "Добавить напрямую", exact: true }).isDisabled(), false);
      await addDevice(page, manualMac.toUpperCase()); await checked(page, manualMac, true);
      await checked(page, offlineMac, true);
      assert.deepEqual(state.status.device_exclusions, [offlineMac, manualMac]);
      assert.deepEqual(state.status.routing.config, before);
      assert.equal(state.requests.filter(req => req.endpoint === "POST /api/routing").length, 0);
      await screenshot(page, `devices-discovery-${discovery}-${width}-${theme}`);
      await close(test, `devices discovery ${discovery}: offline saved retained, manual usable, config preserved ${width}px`);
    }
  }

  let test = await open(browser, "/#/routing", 390);
  let { page, state } = test;
  state.status.device_exclusions = [pickedMac, offlineMac];
  await page.reload(); await checked(page, offlineMac, true);
  for (const label of ["Весь интернет", "По правилам"]) {
    await page.getByRole("button", { name: label, exact: true }).click();
    await page.locator(`button[aria-pressed="true"]`).getByText(label, { exact: true }).waitFor();
    await page.waitForFunction(() => !document.querySelector('[aria-label="Напрямую: Ноутбук"]').disabled);
    assert.deepEqual(state.status.device_exclusions, [pickedMac, offlineMac]);
  }
  await page.getByRole("button", { name: "Добавить", exact: true }).click();
  await page.getByLabel("Название", { exact: true }).fill("Exclusion preservation");
  await page.getByLabel("Значение", { exact: false }).fill("example.com");
  await page.getByRole("button", { name: "Сохранить", exact: true }).click();
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  await page.getByRole("switch", { name: "Включить правило Exclusion preservation", exact: true }).click();
  await page.reload(); await checked(page, offlineMac, true); await checked(page, "Ноутбук", true);
  assert.equal(state.status.routing.config.domain_rules[0].matcher.value, "example.com");
  assert.equal(state.status.routing.config.domain_rules[0].enabled, false);
  assert.deepEqual(state.status.device_exclusions, [pickedMac, offlineMac]);
  assert.equal(state.requests.filter(req => req.endpoint === "POST /api/routing").length, 4);
  await page.goto(`${origin}/#/`);
  await page.getByRole("button", { name: "Отключить VPN", exact: true }).click();
  await page.getByRole("button", { name: "Отключить", exact: true }).click();
  await page.getByRole("heading", { name: "VPN отключён", exact: true }).waitFor();
  await page.goto(`${origin}/#/routing`); await checked(page, "Ноутбук", true);
  assert.deepEqual(state.status.device_exclusions, [pickedMac, offlineMac]);
  assert.equal(devicePosts(state).length, 0);
  await screenshot(page, "devices-rules-mode-preserved-390");
  await close(test, "devices preserved by ordinary rule pack/save/toggle, routing modes, VPN off and reload");

  for (const session of ["logout", "expiry"]) {
    test = await open(browser, "/#/routing", 390); ({ page, state } = test);
    state.status.device_exclusions = [offlineMac];
    await page.reload(); await checked(page, offlineMac, true);
    if (session === "logout") {
      await page.goto(`${origin}/#/system`);
      await page.getByRole("button", { name: "Выйти", exact: true }).click();
    } else {
      state.auth = { state: "login", csrf_token: state.auth.csrf_token };
      state.fail = { endpoint: "GET /api/status", status: 401, error: "session_expired" };
    }
    await page.getByRole("heading", { name: "Вход в панель", exact: true, level: 1 }).waitFor();
    assert.equal(await devicePanel(page).count(), 0);
    await page.locator('input[autocomplete="current-password"]').fill(adminPassword);
    await page.getByRole("button", { name: "Войти", exact: true }).click();
    await page.locator(".app-main .panel").first().waitFor();
    await page.goto(`${origin}/#/routing`); await checked(page, offlineMac, true);
    assert.deepEqual(state.status.device_exclusions, [offlineMac]);
    assert.equal(devicePosts(state).length, 0);
    await screenshot(page, `devices-session-${session}-390`);
    await close(test, `devices ${session}: authenticated UI hidden, login/return renders saved offline exclusions`);
  }
  assert.equal(passed - baseline, 24, "all new device scenarios ran");
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
  if (name === "app.js") assert.doesNotMatch(bytes.toString(), /8443/, "production client must not hardcode legacy port");
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
      assert.equal(await page.getByText(/Wi-Fi|Диагностика|Перезагрузить|Температура|Домашний роутер/).count(), 0);
      assert.equal(await page.getByRole("heading", { name: /Устройства напрямую/ }).count(), label === "Правила" ? 1 : 0);
      assert.equal(await page.locator(".sidebar, .mobile-nav").getByText(/Устройства|DHCP/).count(), 0);
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
  assert.equal(passed, 35, "all original browser baselines retained");
  await passwordChecks(browser);
  assert.equal(passed, 45, "all password/VPS browser baselines retained");
  await deviceChecks(browser);
  console.log(`PASS: ${passed} browser scenarios; ${screenshots} screenshots; 3 asset hashes/gzip verified. All requests mocked at ${origin}.`);
} finally { await browser.close(); }
