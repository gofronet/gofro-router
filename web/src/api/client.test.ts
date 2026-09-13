import assert from "node:assert/strict";
import { ApiError, http, setCsrfToken, clearCsrfToken } from "./client";

declare function test(name: string, run: () => Promise<void>): void;

test("ordinary POST deadline covers stalled headers and body with one write and unknown outcome", async () => {
  const original = globalThis.fetch;
  setCsrfToken("csrf-test");
  try {
    for (const phase of ["headers", "body"]) {
      let calls = 0;
      globalThis.fetch = async (_url, init) => {
        calls++;
        assert.equal(init?.method, "POST");
        assert.equal(init?.credentials, "same-origin");
        assert.equal(new Headers(init?.headers).get("X-CSRF-Token"), "csrf-test");
        const signal = init?.signal;
        assert(signal);
        if (phase === "headers") return new Promise<Response>((_resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
        return new Response(new ReadableStream({ start(controller) {
          controller.enqueue(new TextEncoder().encode('{"version":'));
          signal.addEventListener("abort", () => controller.error(signal.reason), { once: true });
        } }));
      };
      await assert.rejects(http.post("/servers/restart", {}, { timeout: 0 }), error => error instanceof ApiError && error.outcome === undefined && /Результат операции неизвестен/.test(error.message));
      assert.equal(calls, 1);
    }
  } finally { globalThis.fetch = original; clearCsrfToken(); }
});

test("error metadata is optional, validated, and never overrides auth/CSRF rejection", async () => {
  const original = globalThis.fetch;
  try {
    for (const status of [401, 403, 500]) {
      for (const outcome of [undefined, "committed", "unknown", 42]) {
        globalThis.fetch = async () => Response.json({ error: "safe error", outcome }, { status });
        await assert.rejects(http.post("/routing", {}), error => error instanceof ApiError && error.status === status && error.outcome === (status === 500 && outcome === "committed" ? "committed" : undefined));
      }
    }
  } finally { globalThis.fetch = original; }
});
