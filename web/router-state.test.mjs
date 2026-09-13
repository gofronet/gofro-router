import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { plugin } from "bun";
import { test } from "bun:test";
import { compile, compileModule } from "svelte/compiler";
import { render } from "svelte/server";
import { setAppContext } from "./src/app-context.ts";
import { api, ApiError } from "./src/api/index.ts";
import { setCsrfToken, clearCsrfToken } from "./src/api/client.ts";
import { statusSchema } from "./src/api/schemas.ts";

plugin({ name: "svelte-store-test", setup(build) {
  build.onLoad({ filter: /device-exclusions\.svelte$/ }, async ({ path }) => ({
    contents: compile(await readFile(path, "utf8"), { filename: path, generate: "server" }).js.code,
    loader: "js",
  }));
  build.onLoad({ filter: /router-state\.svelte\.ts$/ }, async ({ path }) => ({
    contents: compileModule(new Bun.Transpiler({ loader: "ts" }).transformSync(await readFile(path, "utf8")), { filename: path, generate: "server" }).js.code,
    loader: "js",
  }));
} });
const { RouterState } = await import("./src/stores/router-state.svelte.ts");
const { default: DeviceExclusions } = await import("./src/components/device-exclusions.svelte");
const status = statusSchema.parse({
  version: "test", update: { running: false, result: null }, vpn_enabled: false,
  tunnel_active: false, interface: "wg0", active_server_key: null, servers: [], peer: null,
  stats: { rx_bps: 0, tx_bps: 0 }, history: [],
  routing: { config: { domain_rules: [], ip_rules: [], default_target: "direct" }, dns_active: false, fake_ips: 0, geosite_loaded: false, geoip_loaded: false, dataplane_active: false },
});
const auth = { state: "authenticated", csrf_token: "new-session" };
const onboarding = { step: "complete", networks: [], setup_window_seconds: null, error: null };
function deferred() { return Promise.withResolvers(); }

test("password rotation preserves polling/status, rejects double submit and stale auth replies", () => scenario(async (app, timers) => {
  await app.loginAuth("old-password");
  await app.refresh();
  const generation = app.generation;
  const timer = [...timers.keys()][0];
  const snapshot = app.status;
  const pending = deferred();
  let writes = 0;
  api.auth.changePassword = () => { writes++; return pending.promise; };
  const oldPoll = deferred();
  api.status.get = () => oldPoll.promise;
  const polling = app.refresh();
  const changing = app.changePassword("old-password", "new-password");
  assert.equal(app.busy, true);
  assert.equal(await app.changePassword("old-password", "new-password"), false);
  pending.resolve({ state: "authenticated", csrf_token: "rotated" });
  assert.equal(await changing, true);
  oldPoll.reject(new ApiError("session_expired", 401));
  await polling;
  assert.equal(app.authState, "authenticated");
  assert.equal(app.status, snapshot);
  assert.equal(app.pollError, "");
  assert.equal(app.generation, generation);
  assert.equal([...timers.keys()][0], timer);
  assert.equal(app.statusUncertain, false);
  assert.equal(app.busy, false);
  assert.equal(writes, 1);

  for (const fail of [false, true]) {
    const old = deferred();
    api.auth.status = () => old.promise;
    const initializing = app.initializeAuth();
    api.auth.changePassword = async () => ({ state: "authenticated", csrf_token: "newer" });
    assert.equal(await app.changePassword("old-password", "new-password"), true);
    if (fail) old.reject(new ApiError("session_expired", 401));
    else old.resolve({ state: "login", csrf_token: "stale" });
    await initializing;
    assert.equal(app.authState, "authenticated");
    assert.equal(app.auth.csrf_token, "newer");
    assert.equal(app.authLoading, false);
  }
}));

test("password rotation rejects stale routing success/401 while the current session keeps polling", () => scenario(async (app, timers) => {
  const originalTest = api.routing.test;
  try {
    await app.loginAuth("old-password");
    await app.refresh();
    const generation = app.generation;
    const [timer, tick] = [...timers.entries()][0];
    const rotated = { state: "authenticated", csrf_token: "rotated-route-session" };
    api.auth.changePassword = async () => rotated;
    for (const fail of [true, false]) {
      const pending = deferred();
      api.routing.test = () => pending.promise;
      const routing = app.testRouting("example.com");
      const rejected = assert.rejects(routing, fail ? /session_expired/ : /Request superseded/);
      assert.equal(await app.changePassword("old-password", "new-password"), true);
      if (fail) pending.reject(new ApiError("session_expired", 401));
      else pending.resolve({ value: "example.com", target: "direct", matched_rule: null });
      await rejected;
      assert.equal(app.authState, "authenticated");
      assert.deepEqual(app.auth, rotated);
      assert.equal(app.generation, generation);
      assert.equal(timers.get(timer), tick);
      assert.equal(app.hasStatus, true);
      assert.equal(app.authError, "");
      const polled = deferred();
      api.status.get = async () => { polled.resolve(); return { ...status, version: `after-rotation-${fail}` }; };
      tick();
      await polled.promise;
      assert.equal(app.status.version, `after-rotation-${fail}`);
    }
    api.routing.test = async () => { throw new ApiError("session_expired", 401); };
    await assert.rejects(app.testRouting("example.com"), /session_expired/);
    assert.equal(app.authState, "login", "a current-session 401 still expires the session");
    assert.equal(timers.size, 0);
  } finally { api.routing.test = originalTest; }
}));

test("wrong current password keeps the session; unknown rotation guides the next login", () => scenario(async app => {
  await app.loginAuth("old-password");
  await app.refresh();
  api.auth.changePassword = async () => { throw new ApiError("invalid_current_password", 400); };
  await assert.rejects(app.changePassword("wrong", "new-password"), /Неверный текущий пароль/);
  assert.equal(app.authState, "authenticated");
  assert.equal(app.statusUncertain, false);
  assert.equal(app.busy, false);
  api.auth.changePassword = async () => { throw new ApiError("timeout"); };
  await assert.rejects(app.changePassword("old-password", "new-password"), /попробуйте новый пароль/);
  api.status.get = async () => { throw new ApiError("session_expired", 401); };
  await app.refresh();
  assert.equal(app.authState, "login");
  assert.match(app.authError, /попробуйте новый пароль/);
}));

test("password transport serializes body settlement and resyncs rotated CSRF without replay", async () => {
  const originalFetch = globalThis.fetch;
  const originalTimeout = globalThis.setTimeout;
  try {
    for (const mode of ["success", "wrong-current", "invalid-body", "body-timeout"]) {
      const body = deferred();
      const arrived = deferred();
      const calls = [];
      let cookieCsrf = "old-csrf"; // Simulated browser cookie rotation at response headers.
      globalThis.setTimeout = (callback, ms, ...args) => originalTimeout(callback, ms === 30_000 && mode === "body-timeout" ? 20 : ms, ...args);
      globalThis.fetch = async (url, init) => {
        calls.push(url);
        assert.equal(init.credentials, "same-origin");
        if (url === "/api/auth/status") return Response.json({ state: "authenticated", csrf_token: cookieCsrf });
        assert.equal(new Headers(init.headers).get("X-CSRF-Token"), cookieCsrf);
        if (url === "/api/auth/password") {
          assert.equal(init.method, "POST");
          assert.deepEqual(JSON.parse(init.body), { current_password: "old-password", password: "new-password" });
          cookieCsrf = mode === "wrong-current" ? "old-csrf" : "rotated-csrf";
          arrived.resolve();
          return new Response(new ReadableStream({ start(controller) {
            init.signal.addEventListener("abort", () => controller.error(init.signal.reason), { once: true });
            void body.promise.then(() => {
              controller.enqueue(new TextEncoder().encode(JSON.stringify(mode === "success" ? { state: "authenticated", csrf_token: cookieCsrf } : mode === "wrong-current" ? { error: "invalid_current_password" } : { state: "authenticated" })));
              controller.close();
            });
          } }), { status: mode === "wrong-current" ? 400 : 200 });
        }
        assert.equal(url, "/api/auth/logout");
        return Response.json({ state: "login", csrf_token: "logged-out" });
      };
      await api.auth.status();
      calls.length = 0;
      const changing = api.auth.changePassword("old-password", "new-password").catch(error => error);
      await arrived.promise;
      const logout = api.auth.logout();
      await Promise.resolve();
      assert.deepEqual(calls, ["/api/auth/password"]);
      if (mode !== "body-timeout") body.resolve();
      const result = await changing;
      if (mode === "success") assert.equal(result.state, "authenticated");
      else assert.ok(result instanceof ApiError);
      await logout;
      assert.deepEqual(calls, mode === "success" ? ["/api/auth/password", "/api/auth/logout"] : ["/api/auth/password", "/api/auth/status", "/api/auth/logout"]);
    }
  } finally { globalThis.fetch = originalFetch; globalThis.setTimeout = originalTimeout; clearCsrfToken(); }
});

async function scenario(run) {
  const original = { exclusions: api.deviceExclusions.set, inventory: api.lanDevices.get, status: api.status.get, auth: { ...api.auth }, onboarding: api.onboarding.get, save: api.routing.save, create: api.servers.createFriend, window: globalThis.window };
  const timers = new Map();
  let next = 0;
  globalThis.window = { setInterval(callback) { timers.set(++next, callback); return next; }, clearInterval(id) { timers.delete(id); } };
  api.status.get = async () => structuredClone(status);
  api.auth.login = async () => auth;
  api.auth.logout = async () => ({ state: "login", csrf_token: "logged-out" });
  api.onboarding.get = async () => onboarding;
  const app = new RouterState();
  try { await run(app, timers); }
  finally {
    app.stop();
    api.status.get = original.status;
    api.deviceExclusions.set = original.exclusions;
    api.lanDevices.get = original.inventory;
    Object.assign(api.auth, original.auth);
    api.onboarding.get = original.onboarding;
    api.routing.save = original.save;
    api.servers.createFriend = original.create;
    globalThis.window = original.window;
  }
}

test("device exclusions use authoritative replies, reject stale polls and never replay uncertain writes", () => scenario(async app => {
  await app.loginAuth("password"); await app.refresh();
  const mac = "02:ab:cd:ef:01:23";
  const old = deferred();
  api.status.get = () => old.promise;
  const polling = app.refresh();
  const pending = deferred();
  let writes = 0;
  api.deviceExclusions.set = input => { writes++; assert.deepEqual(input, { mac, excluded: true }); return pending.promise; };
  const writing = app.setDeviceExcluded({ mac: mac.toUpperCase(), excluded: true });
  assert.deepEqual(app.status.device_exclusions, []);
  assert.equal(await app.setDeviceExcluded({ mac, excluded: true }), false);
  pending.resolve({ ...status, device_exclusions: [mac, "02:00:00:00:00:02"] });
  assert.equal(await writing, true);
  old.resolve(status); await polling;
  assert.equal(app.status.device_exclusions.length, 2);
  assert.equal(writes, 1);
  for (const failure of [new ApiError("rejected", 400), new ApiError("timeout"), new ApiError("refresh failed", 500, undefined, "committed")]) {
    api.deviceExclusions.set = async () => { writes++; throw failure; };
    assert.equal(await app.setDeviceExcluded({ mac, excluded: false }), false);
    assert.equal(app.status.device_exclusions.length, 2);
    assert.equal(app.statusUncertain, true);
    if (failure.outcome === "committed") assert.ok(app.actionWarning);
    else assert.ok(app.actionError);
    const count = writes;
    assert.equal(await app.setDeviceExcluded({ mac, excluded: false }), false);
    assert.equal(writes, count);
    api.status.get = async () => ({ ...status, device_exclusions: [mac, "02:00:00:00:00:02"] });
    await app.refresh();
  }
  api.status.get = async () => status;
  await app.refresh();
  assert.deepEqual(app.status.device_exclusions, []);
}));

test("inventory failures preserve exclusions and manual writes; late inventory cannot cross auth rotation", () => scenario(async app => {
  await app.loginAuth("password"); await app.refresh();
  const mac = "02:00:00:00:00:01";
  api.lanDevices.get = async () => { throw new ApiError("source unavailable", 503); };
  await app.refreshLanDevices();
  assert.equal(app.lanDevices.discovery, "unavailable");
  assert.ok(app.inventoryError);
  assert.equal(app.statusUncertain, false);
  api.deviceExclusions.set = async () => ({ ...status, device_exclusions: [mac] });
  assert.equal(await app.setDeviceExcluded({ mac, excluded: true }), true);
  await app.refreshLanDevices();
  assert.deepEqual(app.status.device_exclusions, [mac]);
  const html = render(payload => { setAppContext(app); DeviceExclusions(payload, {}); }).body;
  assert.match(html, /02:00:00:00:00:01/);
  assert.match(html, /Список устройств недоступен/);
  assert.match(html, /Нет текущего IP/);
  assert.match(html, /<input[^>]*placeholder="02:ab:cd:ef:01:23"/);
  assert.match(html, /<button class="btn">Добавить напрямую/);
  api.lanDevices.get = async () => ({ devices: [], discovery: "partial" });
  await app.refreshLanDevices();
  assert.equal(app.lanDevices.discovery, "partial");
  assert.deepEqual(app.status.device_exclusions, [mac]);
  for (const fail of [false, true]) {
    const pending = deferred(); api.lanDevices.get = () => pending.promise;
    const reading = app.refreshLanDevices();
    api.auth.changePassword = async () => auth;
    await app.changePassword("old-password", "new-password");
    if (fail) pending.reject(new ApiError("old session", 401));
    else pending.resolve({ devices: [{ mac, name: "Old", addresses: ["192.168.1.2"] }], discovery: "complete" });
    await reading;
    assert.equal(app.authState, "authenticated");
    assert.equal(app.lanDevices.discovery, "partial");
    assert.equal(app.inventoryLoading, false);
  }
  const pending = deferred(); api.deviceExclusions.set = () => pending.promise;
  const writing = app.setDeviceExcluded({ mac, excluded: false });
  await app.logoutAuth(); pending.resolve(status);
  assert.equal(await writing, false);
  assert.equal(app.hasStatus, false);
}));

test("device exclusion transport sends a MAC delta and parses authoritative status", async () => {
  const originalFetch = globalThis.fetch;
  const mac = "02:ab:cd:ef:01:23";
  const calls = [];
  setCsrfToken("test-csrf");
  globalThis.fetch = async (url, init) => {
    calls.push(url);
    if (url === "/api/lan-devices") {
      assert.equal(init.method, "GET");
      return Response.json({ devices: [{ mac: mac.toUpperCase(), name: null, addresses: ["192.168.1.2"] }], discovery: "partial" });
    }
    assert.equal(url, "/api/device-exclusions");
    assert.equal(init.method, "POST");
    assert.equal(new Headers(init.headers).get("X-CSRF-Token"), "test-csrf");
    assert.deepEqual(JSON.parse(init.body), { mac, excluded: true });
    return Response.json({ ...status, device_exclusions: [mac, "02:00:00:00:00:02"] });
  };
  try {
    assert.equal((await api.deviceExclusions.set({ mac: mac.toUpperCase(), excluded: true })).device_exclusions.length, 2);
    assert.equal((await api.lanDevices.get()).devices[0].mac, mac);
    assert.throws(() => api.deviceExclusions.set({ mac: "ff:ff:ff:ff:ff:ff", excluded: true }));
    assert.equal(calls.length, 2);
  } finally { globalThis.fetch = originalFetch; clearCsrfToken(); }
});

test("device exclusion writes need fresh status and enforce the 256 bound without blocking removal", () => scenario(async app => {
  const mac = "02:ff:ff:ff:ff:ff";
  let writes = 0;
  api.deviceExclusions.set = async () => { writes++; return status; };
  app.authState = "authenticated";
  assert.equal(await app.setDeviceExcluded({ mac, excluded: true }), false);
  await app.refresh();
  api.status.get = async () => { throw new ApiError("offline"); };
  await app.refresh();
  assert.equal(await app.setDeviceExcluded({ mac, excluded: true }), false);
  const full = Array.from({ length: 256 }, (_, i) => `02:00:00:00:00:${i.toString(16).padStart(2, "0")}`);
  api.status.get = async () => ({ ...status, device_exclusions: full });
  await app.refresh();
  assert.equal(await app.setDeviceExcluded({ mac, excluded: true }), false);
  assert.match(app.actionError, /256/);
  assert.equal(writes, 0);
  assert.equal(await app.setDeviceExcluded({ mac: full[0], excluded: false }), true);
  assert.equal(writes, 1);
}));

test("logout invalidates pending status success/error and queued polling callbacks", () => scenario(async (app, timers) => {
  for (const fail of [false, true]) {
    const pending = deferred();
    api.status.get = () => pending.promise;
    const refresh = app.refresh();
    app.startPolling();
    const tick = [...timers.values()][0];
    await app.logoutAuth();
    if (fail) pending.reject(new ApiError("old failure", 401)); else pending.resolve(status);
    await refresh;
    tick();
    assert.equal(app.hasStatus, false);
    assert.equal(app.authState, "login");
    assert.equal(app.pollError, "");
    assert.equal(timers.size, 0);
  }
}));

test("old 401 cannot log out a new login or clear its in-flight status", () => scenario(async (app, timers) => {
  const old = deferred();
  api.status.get = () => old.promise;
  const oldRefresh = app.refresh();
  const fresh = deferred();
  let freshCalls = 0;
  api.status.get = () => { freshCalls++; return fresh.promise; };
  assert.equal(await app.loginAuth("password"), true);
  old.reject(new ApiError("session_expired", 401));
  await oldRefresh;
  await app.refresh();
  assert.equal(freshCalls, 1);
  fresh.resolve(status);
  await fresh.promise;
  assert.equal(app.authState, "authenticated");
  assert.equal(app.hasStatus, true);
  assert.equal(timers.size, 1);
}));

test("logout hides login until settlement and invalidates pending auth", () => scenario(async (app, timers) => {
  const initializing = deferred();
  api.auth.status = () => initializing.promise;
  const initialize = app.initializeAuth();
  const exiting = deferred();
  api.auth.logout = () => exiting.promise;
  const logout = app.logoutAuth();
  initializing.resolve(auth);
  await initialize;
  assert.equal(app.authState, "login");
  assert.equal(app.authLoading, true);
  assert.equal(timers.size, 0);
  exiting.reject(new ApiError("old logout", 401));
  await logout;
  assert.equal(app.authLoading, false);
  assert.equal(await app.loginAuth("password"), true);
  assert.equal(app.authState, "authenticated");
  assert.equal(app.actionError, "");
  assert.equal(timers.size, 1);
}));

test("committed/unknown status writes cannot turn stale A or disabled cache into a false no-op", () => scenario(async app => {
  const select = api.servers.select;
  const mode = api.mode.set;
  try {
    for (const outcome of ["committed", undefined]) {
      api.status.get = async () => ({ ...status, active_server_key: "A" });
      await app.refresh();
      const old = deferred();
      api.status.get = () => old.promise;
      const polling = app.refresh();
      const selections = [];
      api.servers.select = async key => {
        selections.push(key);
        throw new ApiError("observation failed", undefined, undefined, outcome);
      };
      assert.equal(await app.selectServer("B"), outcome === "committed");
      old.resolve({ ...status, active_server_key: "A" });
      await polling;
      assert.equal(app.statusUncertain, true);
      assert.equal(await app.selectServer("A"), false);
      assert.match(app.actionError, /новая команда не отправлена/);
      assert.deepEqual(selections, ["B"]);
      api.status.get = async () => { throw new ApiError("read failed"); };
      await app.refresh();
      assert.equal(app.statusUncertain, true);
      api.status.get = async () => ({ ...status, active_server_key: "B" });
      await app.refresh();
      api.servers.select = async key => { selections.push(key); return { ...status, active_server_key: key }; };
      assert.equal(await app.selectServer("A"), true);
      assert.deepEqual(selections, ["B", "A"]);

      const modes = [];
      api.mode.set = async enabled => {
        modes.push(enabled);
        throw new ApiError("observation failed", undefined, undefined, outcome);
      };
      assert.equal(await app.setMode(true), outcome === "committed");
      assert.equal(await app.setMode(false), false);
      assert.match(app.actionError, /новая команда не отправлена/);
      assert.deepEqual(modes, [true]);
      api.status.get = async () => ({ ...status, vpn_enabled: true });
      await app.refresh();
      api.mode.set = async enabled => { modes.push(enabled); return { ...status, vpn_enabled: enabled }; };
      assert.equal(await app.setMode(false), true);
      assert.deepEqual(modes, [true, false]);
      assert.equal(app.statusUncertain, false);
    }
    const pending = deferred();
    api.mode.set = () => pending.promise;
    const enabling = app.setMode(true);
    assert.equal(await app.setMode(false), false, "busy state must precede cached no-op checks");
    pending.resolve({ ...status, vpn_enabled: true });
    await enabling;
    app.stop();
    assert.equal(app.statusUncertain, true, "a stopped lifecycle cannot lend confidence to its successor");
  } finally { api.servers.select = select; api.mode.set = mode; }
}));

test("a current login 401 resynchronizes preauth CSRF before a corrected password", async () => {
  const login = api.auth.login;
  await scenario(async app => {
    const originalFetch = globalThis.fetch;
    api.auth.login = login;
    let calls = 0;
    let reads = 0;
    setCsrfToken("preauth");
    globalThis.fetch = async (url, init) => {
      if (url === "/api/auth/status") {
        reads++;
        return Response.json({ state: "login", csrf_token: "preauth" });
      }
      assert.equal(url, "/api/auth/login");
      assert.equal(new Headers(init.headers).get("X-CSRF-Token"), "preauth");
      return ++calls === 1 ? Response.json({ error: "invalid_password" }, { status: 401 }) : Response.json(auth);
    };
    try {
      assert.equal(await app.loginAuth("wrong"), false);
      assert.equal(await app.loginAuth("correct"), true);
      assert.equal(calls, 2);
      assert.equal(reads, 1);
    } finally { globalThis.fetch = originalFetch; clearCsrfToken(); }
  });
});

test("auth transport queue includes status/logout and applies CSRF before the next login", async () => {
  const originalFetch = globalThis.fetch;
  try {
    for (const operation of [api.auth.status, api.auth.logout]) {
      for (const fail of [false, true]) {
        const first = deferred();
        const calls = [];
        setCsrfToken("initial-csrf");
        globalThis.fetch = async (url, init) => {
          calls.push(url);
          if (calls.length === 1) return first.promise;
          if (url === "/api/auth/status") {
            assert.equal(fail, true);
            return Response.json({ state: "login", csrf_token: "settled-csrf" });
          }
          assert.equal(url, "/api/auth/login");
          assert.equal(new Headers(init.headers).get("X-CSRF-Token"), "settled-csrf");
          return Response.json(auth);
        };
        const pending = operation().catch(error => error);
        const login = api.auth.login("password");
        await new Promise(resolve => setTimeout(resolve, 0));
        assert.equal(calls.length, 1);
        if (fail) first.reject(new TypeError("network failed"));
        else first.resolve(Response.json({ state: "login", csrf_token: "settled-csrf" }));
        await pending;
        assert.deepEqual(await login, auth);
        assert.equal(calls.length, fail ? 3 : 2);
      }
    }
  } finally { globalThis.fetch = originalFetch; clearCsrfToken(); }
});

test("failed CSRF resync stays unverified and refuses credentials until a valid status reply", async () => {
  const originalFetch = globalThis.fetch;
  const originalTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (callback, ms, ...args) => originalTimeout(callback, ms === 10_000 ? 0 : ms, ...args);
  let phase = "verified";
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push(url);
    if (url === "/api/auth/status") {
      if (phase === "http-error") return Response.json({ error: "unavailable" }, { status: 500 });
      if (phase === "invalid-body") return Response.json({ state: "login" });
      if (phase === "headers-timeout") return new Promise((_resolve, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason), { once: true }));
      if (phase === "body-timeout") return new Response(new ReadableStream({ start(controller) {
        init.signal.addEventListener("abort", () => controller.error(init.signal.reason), { once: true });
      } }));
      return Response.json({ state: "login", csrf_token: "verified-csrf" });
    }
    if (url === "/api/auth/logout") return new Response('{"state":', { status: 200 });
    assert.equal(phase, "verified");
    assert.equal(new Headers(init.headers).get("X-CSRF-Token"), "verified-csrf");
    return Response.json(auth);
  };
  try {
    await api.auth.status();
    await assert.rejects(api.auth.logout());
    for (phase of ["http-error", "invalid-body", "headers-timeout", "body-timeout"]) {
      await assert.rejects(api.auth.login("password"));
      assert.equal(calls.filter(url => url === "/api/auth/login").length, 0);
    }
    phase = "verified";
    assert.deepEqual(await api.auth.login("password"), auth);
    assert.equal(calls.filter(url => url === "/api/auth/logout").length, 1);
    assert.equal(calls.filter(url => url === "/api/auth/login").length, 1);
    assert.equal(calls.filter(url => url === "/api/auth/status").length, 6);
  } finally { globalThis.fetch = originalFetch; globalThis.setTimeout = originalTimeout; clearCsrfToken(); }
});

test("stop invalidates pending initialize/login/setup and onboarding, including failures", () => scenario(async (app, timers) => {
  for (const method of ["initializeAuth", "loginAuth", "setupAuth"]) {
    for (const fail of [false, true]) {
      const pending = deferred();
      api.auth.status = api.auth.login = api.auth.setup = () => pending.promise;
      const request = app[method]("password");
      app.stop();
      if (fail) pending.reject(new ApiError("old auth error", 401)); else pending.resolve(auth);
      await request;
      assert.equal(app.authState, "login");
      assert.equal(app.authError, "");
      assert.equal(app.authLoading, false);
      assert.equal(timers.size, 0);
    }
  }
  api.auth.status = async () => auth;
  const pending = deferred();
  api.onboarding.get = () => pending.promise;
  const initializing = app.initializeAuth();
  await Promise.resolve();
  app.stop();
  pending.resolve(onboarding);
  await initializing;
  assert.equal(app.onboarding, null);
  assert.equal(timers.size, 0);
}));

test("mutation/status coordinator rejects older polls and stopped mutation completions", () => scenario(async app => {
  const old = deferred();
  api.status.get = () => old.promise;
  const refresh = app.refresh();
  api.routing.save = async () => ({ ...status, version: "written" });
  assert.equal(await app.saveRouting(status.routing.config), true);
  old.resolve(status);
  await refresh;
  assert.equal(app.status.version, "written");
  const pending = deferred();
  api.routing.save = () => pending.promise;
  const save = app.saveRouting(status.routing.config);
  app.stop();
  pending.reject(new ApiError("old mutation error", 401));
  assert.equal(await save, false);
  assert.equal(app.actionError, "");
  assert.equal(app.busy, false);
}));

test("known commit preserves routing draft and friends warn without replay; failed writes stay failed", () => scenario(async app => {
  let writes = 0;
  api.routing.save = async () => { writes++; throw new ApiError("refresh failed", 500, undefined, "committed"); };
  const previous = status.routing.config;
  let draft = { ...previous, default_target: "block" };
  if (!await app.saveRouting(draft)) draft = previous;
  assert.equal(draft.default_target, "block");
  assert.equal(app.actionError, "");
  assert.notEqual(app.actionWarning, "");
  await app.refresh();
  assert.equal(writes, 1);
  api.servers.createFriend = async () => { writes++; throw new ApiError("refresh failed", 500, undefined, "committed"); };
  assert.equal(await app.createFriend("key", "Friend"), null);
  assert.notEqual(app.actionWarning, "");
  assert.equal(writes, 2);
  api.routing.save = async () => { throw new ApiError("write rejected", 500); };
  assert.equal(await app.saveRouting(draft), false);
  assert.notEqual(app.actionWarning, "");
  assert.equal(app.actionError, "write rejected");
  await app.refresh();
  assert.notEqual(app.actionWarning, "", "local status cannot resolve a managed-server warning");
  const inspect = api.servers.inspect;
  try {
    api.servers.inspect = async () => { throw new ApiError("observation failed", 500); };
    assert.equal(await app.inspectServer("key"), null);
    assert.notEqual(app.actionWarning, "");
    api.servers.inspect = async () => ({ version: "0.5.15", peers: [] });
    await app.inspectServer("other-key");
    assert.notEqual(app.actionWarning, "");
    await app.inspectServer("key");
    assert.equal(app.actionWarning, "");
  } finally { api.servers.inspect = inspect; }
}));

test("managed inspection and write deadlines release the global coordinator after stalled headers/body", () => scenario(async app => {
  const originalFetch = globalThis.fetch;
  const originalTimeout = globalThis.setTimeout;
  try {
    for (const phase of ["headers", "body"]) {
      for (const [operation, deadline] of [
        [() => app.inspectServer("key"), 150_000],
        [() => app.restartServer("key"), 300_000],
        [() => app.updateManagedServer("key"), 1_200_000],
      ]) {
        let calls = 0;
        globalThis.setTimeout = (callback, milliseconds) => {
          assert.equal(milliseconds, deadline);
          return originalTimeout(callback, 0);
        };
        globalThis.fetch = async (_url, init) => {
          calls++;
          const signal = init.signal;
          assert.equal(init.method, "POST");
          if (phase === "headers") return new Promise((_resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
          return new Response(new ReadableStream({ start(controller) {
            signal.addEventListener("abort", () => controller.error(signal.reason), { once: true });
          } }));
        };
        assert.equal(await operation(), null);
        assert.equal(app.busy, false);
        assert.match(app.actionError, /Результат операции неизвестен/);
        assert.equal(app.actionWarning, "");
        assert.equal(calls, 1);
      }
    }
  } finally { globalThis.fetch = originalFetch; globalThis.setTimeout = originalTimeout; }
}));
