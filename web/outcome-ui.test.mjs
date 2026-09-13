// ASSET_DIR=/absolute/build/output PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs bun test outcome-ui.test.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "bun:test";

test.skipIf(!process.env.ASSET_DIR || !process.env.PLAYWRIGHT_MODULE)("all mutation screens retain commit warnings, refresh without replay, and validate Unicode routing names", async () => {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" });
  const key = Buffer.alloc(32, 1).toString("base64");
  try {
    for (const width of [1440, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 900 }, serviceWorkers: "block" });
      const status = {
        version: "0.5.15", update: { running: false, result: null }, vpn_enabled: false,
        tunnel_active: false, interface: "wg0", active_server_key: key,
        servers: [{ name: "VPS", endpoint: "1.1.1.1:8443", public_key: key, managed: true }], peer: null,
        stats: { rx_bps: 0, tx_bps: 0 }, history: [],
        routing: { config: { mode: "rules", rule_order: null, domain_rules: [], ip_rules: [], default_target: "direct" }, dns_active: false, fake_ips: 0, geosite_loaded: false, geoip_loaded: false, dataplane_active: false, degraded: false },
      };
      const peers = [];
      const requests = [];
      const errors = [];
      let inspections = 0;
      let failStatus = false;
      let failInspection = false;
      let step = "complete";
      let unknownWrite = false;
      await context.route("**/*", async route => {
        try {
          const request = route.request();
          const url = new URL(request.url());
          assert.equal(url.origin, "https://gofro.test");
          const json = (data, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(data) });
          if (!url.pathname.startsWith("/api/")) {
            const file = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
            assert(["index.html", "app.js", "app.css", "chart.js"].includes(file));
            return await route.fulfill({ contentType: file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : "text/html", body: await readFile(join(process.env.ASSET_DIR, file)) });
          }
          const endpoint = `${request.method()} ${url.pathname}`;
          requests.push(endpoint);
          if (request.method() !== "GET") assert.equal(request.headers()["x-csrf-token"], "csrf");
          if (endpoint === "GET /api/auth/status") return await json({ state: "authenticated", csrf_token: "csrf" });
          if (endpoint === "GET /api/onboarding") return await json({ step, networks: [], setup_window_seconds: null, error: null });
          if (endpoint === "GET /api/status") return await json(failStatus ? { error: "observation unavailable" } : status, failStatus ? 500 : 200);
          if (endpoint === "POST /api/routing") {
            status.routing.config = request.postDataJSON();
            failStatus = true;
            return await json({ error: "status refresh failed", outcome: "committed" }, 500);
          }
          if (endpoint === "POST /api/servers/management") {
            inspections++;
            if (failInspection) return await json({ error: "inspection unavailable" }, 500);
            return await json({ version: "0.5.15", peers });
          }
          if (endpoint === "POST /api/servers/friends") {
            peers.push({ public_key: Buffer.alloc(32, 2).toString("base64"), name: request.postDataJSON().name, revoked: false, can_share: true });
            return await json({ error: "second SSH status failed", outcome: "committed" }, 500);
          }
          if (["POST /api/mode", "POST /api/update", "POST /api/servers/select", "PUT /api/servers", "DELETE /api/servers", "POST /api/servers/import"].includes(endpoint)) {
            const input = request.postDataJSON();
            if (endpoint === "POST /api/mode") status.vpn_enabled = input.vpn_enabled;
            if (endpoint === "POST /api/update") status.update.running = true;
            if (endpoint === "POST /api/servers/select") status.active_server_key = input.public_key;
            if (endpoint === "PUT /api/servers") Object.assign(status.servers.find(server => server.public_key === input.previous_public_key), input);
            if (endpoint === "DELETE /api/servers") status.servers = status.servers.filter(server => server.public_key !== input.public_key);
            if (endpoint === "POST /api/servers/import") status.servers.push({ name: input.name, endpoint: "1.0.0.1:8443", public_key: Buffer.alloc(32, 3).toString("base64"), managed: false });
            failStatus = true;
            return await json({ error: "status refresh failed", ...(unknownWrite ? {} : { outcome: "committed" }) }, 500);
          }
          throw new Error(`Unexpected request: ${endpoint}`);
        } catch (error) { errors.push(error); await route.abort(); }
      });
      const page = await context.newPage();
      page.on("pageerror", error => errors.push(error));
      const warning = page.getByRole("status").filter({ hasText: /Изменение выполнено, но состояние|Показано последнее известное состояние/ });
      const count = endpoint => requests.filter(value => value === endpoint).length;
      async function refreshOnly(endpoint) {
        await warning.waitFor();
        const writes = count(endpoint);
        assert.equal(await warning.count(), 1);
        assert.equal(await warning.evaluate(node => node.classList.contains("error")), false);
        failStatus = false;
        await warning.getByRole("button", { name: "Обновить состояние", exact: true }).click();
        await warning.waitFor({ state: "hidden" });
        assert.equal(count(endpoint), writes);
      }
      await page.goto("https://gofro.test/#/routing");
      await page.getByRole("button", { name: "Весь интернет", exact: true }).click();
      await page.getByText(/Изменение выполнено, но состояние/).waitFor();
      assert.equal(await page.getByRole("button", { name: "Весь интернет", exact: true }).getAttribute("aria-pressed"), "true");
      await page.getByRole("button", { name: "Добавить", exact: true }).click();
      await page.getByLabel("Значение", { exact: false }).fill("example.com");
      const name = page.getByLabel("Название", { exact: true });
      assert.equal(await name.getAttribute("maxlength"), null);
      for (const invalid of ["😀".repeat(65), "Я".repeat(65), " \u0085\u2003 ", "name\u0007"]) {
        await name.fill(invalid);
        await page.getByRole("button", { name: "Сохранить", exact: true }).click();
        await page.locator("#routing-name-error").waitFor();
        assert.match(await name.getAttribute("aria-describedby"), /routing-name-error/);
        assert.equal(count("POST /api/routing"), 1);
        assert.equal(await page.locator(".rule-row").count(), 0);
      }
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Добавить", exact: true }).click();
      assert.equal(await page.locator("#routing-name-error").count(), 0);
      await name.fill(` \u0085${"😀".repeat(64)}\u2003 `);
      await page.getByLabel("Значение", { exact: false }).fill("example.com");
      await page.getByRole("button", { name: "Сохранить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await page.getByRole("heading", { name: "😀".repeat(64), exact: true }).waitFor();
      assert.equal(status.routing.config.domain_rules[0].name, "😀".repeat(64));
      await page.getByRole("button", { name: `Изменить ${"😀".repeat(64)}`, exact: true }).click();
      await name.fill("Я".repeat(65));
      await page.getByRole("button", { name: "Сохранить", exact: true }).click();
      await page.locator("#routing-name-error").waitFor();
      assert.equal(count("POST /api/routing"), 2);
      assert.equal(await page.locator(".rule-row h3").textContent(), "😀".repeat(64));
      await name.fill(`\u0085${"Я".repeat(64)}\u2003`);
      await page.getByRole("button", { name: "Сохранить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      assert.equal(await page.locator(".rule-row h3").textContent(), "Я".repeat(64));
      assert.equal(status.routing.config.domain_rules[0].name, "Я".repeat(64));
      await refreshOnly("POST /api/routing");
      assert.equal(requests.filter(value => value === "POST /api/routing").length, 3);
      assert.equal(await page.getByRole("alert").count(), 0);
      await page.goto(`https://gofro.test/?server=${encodeURIComponent(key)}#/server-management`);
      await page.getByRole("button", { name: "Создать доступ", exact: true }).click();
      await page.getByLabel("Имя друга").fill("Friend");
      await page.getByRole("dialog").getByRole("button", { name: "Создать доступ", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await page.getByText(/Изменение выполнено, но состояние/).waitFor();
      assert.equal(await page.getByRole("button", { name: "Создать доступ", exact: true }).isDisabled(), true);
      assert.equal(inspections, 1);
      failInspection = true;
      await warning.getByRole("button", { name: "Обновить состояние", exact: true }).click();
      await page.getByText(/Проверка не выполнена/).waitFor();
      assert.equal(await warning.count(), 1, "failed read must not erase the committed outcome");
      await page.locator(width < 920 ? ".mobile-nav" : ".nav-list").getByRole("link", { name: "Панель", exact: true }).click();
      await warning.waitFor();
      failInspection = false;
      await warning.getByRole("link", { name: "Проверить состояние сервера", exact: true }).click();
      await page.getByRole("heading", { name: "Friend", exact: true }).waitFor();
      assert.equal(inspections, 3);
      assert.equal(requests.filter(value => value === "POST /api/servers/friends").length, 1);
      assert.equal(await page.getByRole("alert").count(), 0);

      await page.goto("https://gofro.test/#/system");
      await page.getByRole("button", { name: "Проверить обновления", exact: true }).click();
      await page.getByRole("button", { name: "Проверить и обновить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await warning.waitFor();
      await page.locator(width < 920 ? ".mobile-nav" : ".nav-list").getByRole("link", { name: "Обзор", exact: true }).click();
      await warning.waitFor();
      await warning.getByRole("button", { name: "Обновить состояние", exact: true }).click();
      await page.getByText(/Нет свежих данных/).waitFor();
      assert.equal(await warning.count(), 1);
      await refreshOnly("POST /api/update");
      assert.equal(count("POST /api/update"), 1);
      for (const unknown of [false, true]) {
        unknownWrite = unknown;
        await page.getByRole("button", { name: "Подключить VPN", exact: true }).click();
        await warning.waitFor();
        const writes = count("POST /api/mode");
        await page.getByRole("button", { name: "Подключить VPN", exact: true }).click();
        await page.getByText(/новая команда не отправлена/).waitFor();
        assert.equal(count("POST /api/mode"), writes, "stale toggle is explicitly blocked");
        await refreshOnly("POST /api/mode");
        unknownWrite = false;
        await page.getByRole("button", { name: "Отключить VPN", exact: true }).click();
        await page.getByRole("dialog").getByRole("button", { name: "Отключить", exact: true }).click();
        await page.getByRole("dialog").waitFor({ state: "hidden" });
        await refreshOnly("POST /api/mode");
      }
      assert.equal(count("POST /api/mode"), 4);

      const keyA = Buffer.alloc(32, 4).toString("base64");
      status.servers.push({ name: "Server A", endpoint: "1.0.0.1:8443", public_key: keyA, managed: false });
      status.active_server_key = keyA;
      await page.goto("https://gofro.test/?selection-test#/servers");
      await page.getByRole("button", { name: "Подключить VPS", exact: true }).click();
      await page.getByRole("dialog").getByRole("button", { name: "Подключить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await page.getByRole("button", { name: "Подключить Server A", exact: true }).click();
      await page.getByRole("dialog").getByRole("button", { name: "Подключить", exact: true }).click();
      await page.getByRole("dialog").getByText(/новая команда не отправлена/).waitFor();
      assert.equal(count("POST /api/servers/select"), 1);
      assert.equal(count("POST /api/mode"), 4);
      await page.keyboard.press("Escape");
      await refreshOnly("POST /api/servers/select");
      await page.getByRole("button", { name: "Подключить Server A", exact: true }).click();
      await page.getByRole("dialog").getByRole("button", { name: "Подключить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await refreshOnly("POST /api/servers/select");
      assert.equal(count("POST /api/servers/select"), 2);
      assert.equal(status.active_server_key, keyA);
      assert.equal(count("POST /api/mode"), 4, "selection observation failure must stop the chained mode write");
      status.servers = status.servers.filter(server => server.public_key !== keyA);
      status.active_server_key = key;
      await page.reload();
      await page.getByRole("button", { name: "Настройки VPS", exact: true }).click();
      await page.getByLabel("Название", { exact: true }).fill("Renamed VPS");
      await page.getByRole("button", { name: "Сохранить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await refreshOnly("PUT /api/servers");
      await page.getByRole("heading", { name: "Renamed VPS", exact: true }).waitFor();
      await page.getByRole("button", { name: "Настройки Renamed VPS", exact: true }).click();
      await page.getByRole("dialog").getByRole("button", { name: "Удалить", exact: true }).click();
      await page.getByRole("dialog").getByRole("button", { name: "Удалить", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await refreshOnly("DELETE /api/servers");
      await page.getByRole("heading", { name: "Пока нет серверов", exact: true }).waitFor();

      step = "server";
      await page.reload();
      await page.getByRole("button", { name: "Импортировать настройки" }).click();
      await page.getByLabel("Название", { exact: true }).fill("Imported VPN");
      await page.getByLabel("Или вставьте настройки").fill(`[Interface]\nPrivateKey = ${key}\nAddress = 10.66.0.2/32\n\n[Peer]\nPublicKey = ${Buffer.alloc(32, 3).toString("base64")}\nEndpoint = 1.0.0.1:8443\nAllowedIPs = 0.0.0.0/0\n`);
      await page.getByRole("button", { name: "Импортировать", exact: true }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await refreshOnly("POST /api/servers/import");
      await page.getByRole("button", { name: "Завершить настройку", exact: true }).waitFor();
      assert.equal(count("POST /api/servers/import"), 1);
      assert.deepEqual(errors, []);
      await context.close();
    }
  } finally { await browser.close(); }
}, 60_000);
