import assert from "node:assert/strict";
import { api } from "./index";
import { ApiError, bootstrapStream, clearCsrfToken, readBootstrapStream, setCsrfToken } from "./client";
import { statusSchema, type BootstrapStage } from "./schemas";

declare function test(name: string, run: () => Promise<void>): void;

const status = statusSchema.parse({
  version: "test", update: { running: false, result: null },
  vpn_enabled: false, tunnel_active: false, interface: "wg0", active_server_key: null,
  servers: [{ name: "VPS", endpoint: "203.0.113.1:8443", public_key: "key", managed: true }],
  peer: null, stats: { rx_bps: 0, tx_bps: 0 }, history: [],
  routing: { config: { domain_rules: [], ip_rules: [], default_target: "direct" }, dns_active: false, fake_ips: 0, geosite_loaded: false, geoip_loaded: false, dataplane_active: false },
});
const complete = JSON.stringify({ type: "complete", status });
const encoder = new TextEncoder();

function stream(text: string, bytewise = false): ReadableStream<Uint8Array> {
  const bytes = encoder.encode(text);
  return new ReadableStream({
    start(controller) {
      if (bytewise) for (const byte of bytes) controller.enqueue(Uint8Array.of(byte));
      else controller.enqueue(bytes);
      controller.close();
    },
  });
}

test("bootstrap parses split UTF-8, multiple lines, CRLF and optional skipped stages", async () => {
  const stages: BootstrapStage[] = [];
  const text = `{"type":"stage","stage":"waiting"}\r\n{"type":"stage","stage":"inspect"}\n${complete}\n`;
  assert.deepEqual(await readBootstrapStream(stream(text), stage => stages.push(stage)), status);
  assert.deepEqual(stages, ["waiting", "inspect"]);
  assert.deepEqual(await readBootstrapStream(stream(complete, true), () => {}), status);
  const failure = JSON.stringify({ type: "error", stage: "connect", message: "Неверный пароль root" });
  await assert.rejects(readBootstrapStream(stream(failure, true), stage => stages.push(stage)), { message: "Неверный пароль root" });
  assert.equal(stages.at(-1), "connect");
  const partial = "Local enrollment may already be saved, but activation failed. Remote access was retained. Refresh the server list and repair local routing before retrying.";
  await assert.rejects(readBootstrapStream(stream(JSON.stringify({ type: "error", stage: "save", message: partial })), () => {}), { message: partial });
});

test("bootstrap observation deadline aborts stalled headers/body without retry and clears after completion", async () => {
  const original = globalThis.fetch;
  const input = { name: "VPS", host: "203.0.113.1", port: 22, password: "test-password" };
  try {
    for (const mode of ["headers", "stage", "terminal", "complete"]) {
      let calls = 0;
      let observedSignal: AbortSignal | undefined;
      const stages: BootstrapStage[] = [];
      globalThis.fetch = async (_url, init) => {
        calls++;
        const signal = init?.signal;
        assert(signal);
        observedSignal = signal;
        if (mode === "headers") {
          return new Promise<Response>((_resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
        }
        return new Response(new ReadableStream<Uint8Array>({
          start(controller) {
            controller.enqueue(encoder.encode(mode === "stage" ? '{"type":"stage","stage":"inspect"}\n' : `${complete}\n`));
            if (mode === "complete") controller.close();
            else signal.addEventListener("abort", () => controller.error(signal.reason), { once: true });
          },
        }), { headers: { "Content-Type": "application/x-ndjson" } });
      };
      // A zero deadline runs on the next event-loop turn, without wall-clock timing assertions.
      const operation = bootstrapStream(input, stage => stages.push(stage), 0);
      if (mode === "complete") {
        assert.deepEqual(await operation, status);
        await new Promise(resolve => setTimeout(resolve, 0));
        assert.equal(observedSignal?.aborted, false);
      } else {
        await assert.rejects(operation, /Результат настройки неизвестен.*Настройка на роутере и VPS может продолжаться.*Проверьте список серверов/);
        assert.equal(observedSignal?.aborted, true);
        assert.deepEqual(stages, mode === "stage" ? ["inspect"] : []);
      }
      assert.equal(calls, 1);
    }
  } finally {
    globalThis.fetch = original;
  }
});

test("bootstrap requires exactly one valid terminal event and bounded data", async () => {
  const invalid = [
    "", '{"type":"stage","stage":"waiting"}\n', "not json\n",
    '{"type":"stage","stage":"unknown"}\n', '{"type":"complete","status":{}}\n',
    `${complete}\n${complete}\n`, `${complete}\n{"type":"stage","stage":"save"}\n`,
    `${complete}\n{"type":"error","stage":"save","message":"failed"}\n`,
    JSON.stringify({ type: "error", stage: "save", message: "x".repeat(4097) }),
    "x".repeat(4 * 1024 * 1024 + 1),
  ];
  for (const text of invalid) {
    await assert.rejects(readBootstrapStream(stream(text), () => {}), /Результат настройки неизвестен/);
  }
  const aborted = new ReadableStream<Uint8Array>({ start(controller) { controller.error(new DOMException("aborted", "AbortError")); } });
  await assert.rejects(readBootstrapStream(aborted, () => {}), /Результат настройки неизвестен/);
  const invalidUtf8 = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(Uint8Array.of(0xff)); controller.close(); } });
  await assert.rejects(readBootstrapStream(invalidUtf8, () => {}), /Результат настройки неизвестен/);
});

test("bootstrap POST preserves CSRF and same-origin credentials without probe, host_key or retries", async () => {
  const original = globalThis.fetch;
  let calls = 0;
  let response = new Response(stream(`${complete}\n`), { headers: { "Content-Type": "application/x-ndjson; charset=utf-8" } });
  globalThis.fetch = async (url, init) => {
    calls++;
    assert.equal(url, "/api/servers/bootstrap");
    assert.equal(init?.method, "POST");
    assert.equal(init?.credentials, "same-origin");
    assert.equal(init?.cache, "no-store");
    assert.equal(new Headers(init?.headers).get("X-CSRF-Token"), "csrf-test");
    assert.equal(new Headers(init?.headers).get("Accept"), "application/x-ndjson");
    assert.deepEqual(JSON.parse(String(init?.body)), { name: "VPS", host: "203.0.113.1", port: 22, password: "test-password" });
    return response;
  };
  const send = () => api.servers.bootstrap("VPS", "203.0.113.1", 22, "test-password", () => {});
  setCsrfToken("csrf-test");
  try {
    assert.deepEqual(await send(), status);
    assert.equal(calls, 1);
    response = new Response(stream('{"type":"stage","stage":"save"}\n'), { headers: { "Content-Type": "application/x-ndjson" } });
    await assert.rejects(send(), /Результат настройки неизвестен/);
    assert.equal(calls, 2);
    response = new Response("denied", { status: 403 });
    await assert.rejects(send(), error => error instanceof ApiError && error.status === 403);
    assert.equal(calls, 3);
    response = new Response(complete, { headers: { "Content-Type": "application/json" } });
    await assert.rejects(send(), /Результат настройки неизвестен/);
    assert.equal(calls, 4);
  } finally {
    globalThis.fetch = original;
    clearCsrfToken();
  }
});
