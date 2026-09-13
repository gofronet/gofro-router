import { z, type ZodType } from "zod";
import { bootstrapEventSchema, type BootstrapEvent, type BootstrapStage, type Status } from "./schemas";

const errorResponseSchema = z.object({ error: z.string(), outcome: z.literal("committed").optional() });
let csrfToken = "";

export function setCsrfToken(token: string): void {
  csrfToken = token;
}

export function clearCsrfToken(): void {
  csrfToken = "";
}

type Options = { data?: unknown; timeout?: number };

async function send(
  method: string,
  path: string,
  body?: unknown,
  options: Options = {},
): Promise<{ data: unknown }> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), options.timeout ?? 10_000);

  try {
    const response = await fetch(`/api${path}`, {
      method,
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        ...(method === "GET" ? {} : { "X-CSRF-Token": csrfToken }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: controller?.signal,
      cache: "no-store",
    });
    let data: unknown;
    try {
      data = await response.json();
    } catch (error) {
      if (
        error instanceof TypeError ||
        (error instanceof DOMException && error.name === "AbortError")
      ) {
        throw error;
      }
      data = null;
    }
    if (!response.ok) {
      const parsed = errorResponseSchema.safeParse(data);
      throw new ApiError(
        parsed.success ? parsed.data.error : `Ошибка HTTP ${response.status}`,
        response.status,
        undefined,
        parsed.success && response.status !== 401 && response.status !== 403 ? parsed.data.outcome : undefined,
      );
    }
    return { data };
  } catch (error) {
    if (controller.signal.aborted || error instanceof TypeError) {
      throw new ApiError(method === "GET" ? "Устройство не отвечает" : "Результат операции неизвестен. Проверьте состояние перед повторной попыткой. Запрос не повторён автоматически.", undefined, error);
    }
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

export const http = {
  get: (path: string) => send("GET", path),
  post: (path: string, body: unknown, options?: Options) =>
    send("POST", path, body, options),
  put: (path: string, body: unknown, options?: Options) =>
    send("PUT", path, body, options),
  delete: (path: string, options?: Options) =>
    send("DELETE", path, options?.data, options),
};

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly cause?: unknown,
    readonly outcome?: "committed",
  ) {
    super(message);
    this.name = "ApiError";
  }
}

const unknownBootstrapOutcome = "Результат настройки неизвестен: ответ не получен вовремя или его не удалось проверить. Настройка на роутере и VPS может продолжаться. Проверьте список серверов, состояние VPN на роутере и состояние VPS перед ручной повторной попыткой. Запрос не будет повторён автоматически.";

export async function readBootstrapStream(
  body: ReadableStream<Uint8Array>,
  onStage: (stage: BootstrapStage) => void,
): Promise<Status> {
  const reader = body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let pending = "";
  let bytes = 0;
  let events = 0;
  let terminal: Exclude<BootstrapEvent, { type: "stage" }> | undefined;
  const line = (value: string) => {
    if (value.length > 4 * 1024 * 1024 || ++events > 64 || terminal) throw new Error("Invalid stream");
    const event = bootstrapEventSchema.parse(JSON.parse(value));
    if (event.type === "stage") onStage(event.stage);
    else terminal = event;
  };
  try {
    while (true) {
      const { value, done } = await reader.read();
      bytes += value?.byteLength ?? 0;
      if (bytes > 8 * 1024 * 1024) throw new Error("Stream too large");
      pending += decoder.decode(value, { stream: !done });
      let end: number;
      while ((end = pending.indexOf("\n")) !== -1) {
        line(pending.slice(0, end));
        pending = pending.slice(end + 1);
      }
      if (pending.length > 4 * 1024 * 1024) throw new Error("Line too large");
      if (done) break;
    }
    if (pending) line(pending);
    if (!terminal) throw new Error("Missing terminal event");
  } catch {
    // Never surface raw parser errors: they can include response data.
    throw new ApiError(unknownBootstrapOutcome);
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
  if (terminal.type === "error") {
    onStage(terminal.stage);
    throw new ApiError(terminal.message);
  }
  return terminal.status;
}

export async function bootstrapStream(
  input: { name: string; host: string; port: number; password: string },
  onStage: (stage: BootstrapStage) => void,
  observationTimeoutMs = 30 * 60_000,
): Promise<Status> {
  // Allow the backend's 20-minute bootstrap and follow-up SSH work; abort only observation.
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), observationTimeoutMs);
  try {
    const response = await fetch("/api/servers/bootstrap", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", Accept: "application/x-ndjson", "X-CSRF-Token": csrfToken },
      body: JSON.stringify(input),
      signal: controller.signal,
      cache: "no-store",
    });
    if (!response.ok) {
      await response.body?.cancel();
      throw new ApiError(response.status === 401 ? "Сессия истекла. Войдите снова." : response.status === 403 ? "Проверка безопасности не пройдена. Обновите страницу перед повторной попыткой." : `Ошибка HTTP ${response.status}. ${unknownBootstrapOutcome}`, response.status);
    }
    if (!response.body || response.headers.get("Content-Type")?.split(";")[0].trim().toLowerCase() !== "application/x-ndjson") {
      await response.body?.cancel();
      throw new ApiError(unknownBootstrapOutcome);
    }
    return await readBootstrapStream(response.body, onStage);
  } catch (error) {
    if (error instanceof ApiError) throw error;
    throw new ApiError(unknownBootstrapOutcome);
  } finally {
    clearTimeout(timeout);
  }
}

function apiError(error: unknown): ApiError {
  if (error instanceof ApiError) return error;

  return new ApiError(
    (error instanceof DOMException && error.name === "AbortError") ||
      error instanceof TypeError
      ? "Устройство не отвечает"
      : error instanceof Error
        ? error.message
        : "Неизвестная ошибка",
    undefined,
    error,
  );
}

export async function request<T>(
  schema: ZodType<T>,
  requestFactory: () => Promise<{ data: unknown }>,
): Promise<T> {
  try {
    const response = await requestFactory();
    const result = schema.safeParse(response.data);
    if (!result.success) {
      throw new ApiError(
        "Сервер вернул данные в неожиданном формате",
        undefined,
        result.error,
      );
    }
    return result.data;
  } catch (error) {
    throw apiError(error);
  }
}
