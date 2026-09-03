import { z, type ZodType } from "zod";

const errorResponseSchema = z.object({ error: z.string() });

type Options = { data?: unknown; timeout?: number };

async function send(
  method: string,
  path: string,
  body?: unknown,
  options: Options = {},
): Promise<{ data: unknown }> {
  const controller = options.timeout === 0 ? undefined : new AbortController();
  const timeout = controller
    ? window.setTimeout(() => controller.abort(), options.timeout ?? 10_000)
    : undefined;

  try {
    const response = await fetch(`/api${path}`, {
      method,
      headers: { "Content-Type": "application/json" },
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
      );
    }
    return { data };
  } finally {
    if (timeout !== undefined) window.clearTimeout(timeout);
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
  ) {
    super(message);
    this.name = "ApiError";
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
