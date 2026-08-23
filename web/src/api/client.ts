import axios, { AxiosError } from "axios";
import { z, type ZodType } from "zod";

const errorResponseSchema = z.object({ error: z.string() });

export const http = axios.create({
  baseURL: "/api",
  headers: { "Content-Type": "application/json" },
  timeout: 10_000,
});

export const updaterHttp = axios.create({
  baseURL: `http://${location.hostname}:8080/api`,
  timeout: 10_000,
});

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

  if (error instanceof AxiosError) {
    const parsed = errorResponseSchema.safeParse(error.response?.data);
    return new ApiError(
      parsed.success
        ? parsed.data.error
        : error.response
          ? `Ошибка HTTP ${error.response.status}`
          : "Устройство не отвечает",
      error.response?.status,
      error,
    );
  }

  return new ApiError(
    error instanceof Error ? error.message : "Неизвестная ошибка",
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
