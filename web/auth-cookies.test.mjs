// Uses the real browser cookie jar and loopback HTTP, never a fetch/API stub.
// ASSET_DIR=/absolute/build/output PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs bun test auth-cookies.test.mjs
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "bun:test";

test.skipIf(!process.env.ASSET_DIR || !process.env.PLAYWRIGHT_MODULE)("real rotated cookies require validated CSRF resync before login after incomplete/error auth replies", async () => {
  const { chromium } = await import(process.env.PLAYWRIGHT_MODULE);
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROME_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" });
  const status = {
    version: "0.5.15", update: { running: false, result: null }, vpn_enabled: false,
    tunnel_active: false, interface: "wg0", active_server_key: null, servers: [], peer: null,
    stats: { rx_bps: 0, tx_bps: 0 }, history: [],
    routing: { config: { mode: "rules", rule_order: null, domain_rules: [], ip_rules: [], default_target: "direct" }, dns_active: false, fake_ips: 0, geosite_loaded: false, geoip_loaded: false, dataplane_active: false, degraded: false },
  };
  const cookie = (name, value, extra = "") => `__Host-gofro-${name}=${value}; Path=/; Secure; HttpOnly; SameSite=Strict${extra}`;
  try {
    for (const mode of ["success", "http-failure", "headers-timeout", "body-timeout", "truncated-body", "invalid-body", "resync-failure"]) {
      const arrived = Promise.withResolvers();
      const closed = Promise.withResolvers();
      let logoutResponse;
      let loginCalls = 0;
      let logoutCalls = 0;
      let statusCalls = 0;
      let verifiedCsrf = "old-csrf";
      const errors = [];
      const logoutHeaders = { "Content-Type": "application/json", "Set-Cookie": [cookie("session", "", "; Max-Age=0"), cookie("csrf", "logout-csrf")] };
      const server = createServer(async (request, response) => {
        try {
          const path = new URL(request.url, "http://localhost").pathname;
          const json = (data, cookies = [], code = 200) => {
            response.writeHead(code, { "Content-Type": "application/json", "Set-Cookie": cookies, "Cache-Control": "no-store" });
            response.end(JSON.stringify(data));
          };
          if (path === "/test/session") return json({}, [cookie("session", "old-session"), cookie("csrf", "old-csrf")]);
          if (path === "/api/auth/status") {
            statusCalls++;
            if (statusCalls === 1) {
              assert.match(request.headers.cookie ?? "", /__Host-gofro-session=old-session/);
              return json({ state: "authenticated", csrf_token: "old-csrf" }, [cookie("csrf", "old-csrf")]);
            }
            const previousCsrf = statusCalls === 2 ? mode === "headers-timeout" ? "old-csrf" : "logout-csrf" : `resync-csrf-${statusCalls - 1}`;
            assert.match(request.headers.cookie ?? "", new RegExp(`__Host-gofro-csrf=${previousCsrf}(?:;|$)`));
            // Like backend status without a valid session, this read rotates CSRF again.
            const csrf = `resync-csrf-${statusCalls}`;
            if (mode === "resync-failure" && statusCalls === 2) return json({ error: "resync_failed" }, [cookie("csrf", csrf)], 500);
            verifiedCsrf = csrf;
            return json({ state: "login", csrf_token: csrf }, [cookie("csrf", csrf)]);
          }
          if (path === "/api/onboarding") return json({ step: "complete", networks: [], setup_window_seconds: null, error: null });
          if (path === "/api/status") {
            assert.match(request.headers.cookie ?? "", new RegExp(`__Host-gofro-session=${loginCalls ? "new-session" : "old-session"}`));
            return json(status);
          }
          if (path === "/api/auth/logout") {
            logoutCalls++;
            assert.equal(request.method, "POST");
            assert.equal(request.headers["x-csrf-token"], "old-csrf");
            request.resume();
            logoutResponse = response;
            response.on("close", () => closed.resolve());
            if (mode === "body-timeout") {
              response.writeHead(200, logoutHeaders);
              response.write('{"state":"login",');
            }
            arrived.resolve();
            return;
          }
          if (path === "/api/auth/login") {
            loginCalls++;
            assert.equal(logoutResponse.writableEnded || logoutResponse.destroyed, true, "login transport must not overtake logout");
            assert.equal(request.headers["x-csrf-token"], verifiedCsrf);
            assert.match(request.headers.cookie ?? "", new RegExp(`__Host-gofro-csrf=${verifiedCsrf}(?:;|$)`));
            assert.equal(statusCalls, mode === "success" ? 1 : mode === "resync-failure" ? 3 : 2);
            request.resume();
            return json({ state: "authenticated", csrf_token: "new-csrf" }, [cookie("session", "new-session"), cookie("csrf", "new-csrf")]);
          }
          const file = path === "/" ? "index.html" : path.slice(1);
          if (!["index.html", "app.js", "app.css", "chart.js"].includes(file)) { response.writeHead(404); response.end(); return; }
          response.writeHead(200, { "Content-Type": file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : "text/html" });
          response.end(await readFile(join(process.env.ASSET_DIR, file)));
        } catch (error) { errors.push(error); response.destroy(); }
      });
      await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
      const origin = `http://127.0.0.1:${server.address().port}`;
      const context = await browser.newContext({ serviceWorkers: "block" });
      try {
        await context.route("**/*", route => new URL(route.request().url()).origin === origin ? route.continue() : route.abort());
        if (mode.endsWith("timeout")) {
          // Accelerate only the deadline; fetch, HTTP bodies and Set-Cookie remain native.
          await context.addInitScript(() => {
            const timeout = window.setTimeout.bind(window);
            window.setTimeout = (callback, ms, ...args) => timeout(callback, ms === 30_000 ? 250 : ms, ...args);
          });
        }
        const page = await context.newPage();
        page.on("pageerror", error => errors.push(error));
        await page.goto(`${origin}/test/session`);
        await page.goto(`${origin}/#/system`);
        await page.getByRole("button", { name: "Выйти", exact: true }).click();
        await arrived.promise;
        assert.equal(await page.locator('input[autocomplete="current-password"]').count(), 0, "logout must not expose the login form");
        assert.equal(loginCalls, 0);
        if (!mode.endsWith("timeout")) {
          logoutResponse.writeHead(mode === "http-failure" ? 500 : 200, { ...logoutHeaders, ...(mode === "truncated-body" ? { "Content-Length": "200", Connection: "close" } : {}) });
          if (mode === "success") verifiedCsrf = "logout-csrf";
          logoutResponse.end(mode === "truncated-body" ? '{"state":"login",' : JSON.stringify(mode === "success" ? { state: "login", csrf_token: "logout-csrf" } : mode === "http-failure" ? { error: "logout_failed" } : { state: "login" }));
        } else {
          await closed.promise;
          assert.equal(logoutResponse.writableEnded, false, "timeout must abort the actual transport");
        }
        await page.getByRole("heading", { name: "Вход в панель", exact: true }).waitFor();
        assert.equal((await context.cookies()).find(value => value.name === "__Host-gofro-csrf")?.value, mode === "headers-timeout" ? "old-csrf" : "logout-csrf");
        await page.getByLabel("Пароль", { exact: true }).fill("correct-password");
        await page.getByRole("button", { name: "Войти", exact: true }).click();
        if (mode === "resync-failure") {
          await page.getByRole("alert").getByText("resync_failed", { exact: true }).waitFor();
          assert.equal(loginCalls, 0, "failed resync must not submit credentials");
          assert.equal((await context.cookies()).find(value => value.name === "__Host-gofro-csrf")?.value, "resync-csrf-2");
          // Only a new explicit submission retries the read; no write was replayed.
          await page.getByRole("button", { name: "Войти", exact: true }).click();
        }
        await page.getByRole("heading", { name: "Обновления Gofro", exact: true }).waitFor();
        if (mode.endsWith("timeout")) {
          // The abandoned server handler tries to clear cookies after the new login.
          if (!logoutResponse.headersSent) logoutResponse.writeHead(200, logoutHeaders);
          logoutResponse.end('{"state":"login","csrf_token":"logout-csrf"}');
        }
        assert.equal(await page.evaluate(async () => (await fetch("/api/status")).status), 200);
        const cookies = await context.cookies();
        assert.equal(cookies.find(value => value.name === "__Host-gofro-session")?.value, "new-session");
        assert.equal(cookies.find(value => value.name === "__Host-gofro-csrf")?.value, "new-csrf");
        assert.equal(loginCalls, 1);
        assert.equal(logoutCalls, 1);
        assert.equal(statusCalls, mode === "success" ? 1 : mode === "resync-failure" ? 3 : 2);
        assert.deepEqual(errors, []);
      } finally {
        await context.close();
        server.closeAllConnections();
        await new Promise(resolve => server.close(resolve));
      }
    }
  } finally { await browser.close(); }
}, 60_000);
