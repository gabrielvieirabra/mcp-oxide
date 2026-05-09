import type {
  Adapter,
  CreateAdapterPayload,
  CreateToolPayload,
  DeploymentStatus,
  Health,
  JsonRpcRequest,
  JsonRpcResponse,
  Tool
} from "./types";

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly payload?: unknown
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export type ApiConfig = {
  baseUrl: string;
  token: string;
};

const trimBaseUrl = (baseUrl: string) => baseUrl.replace(/\/+$/, "");

async function parseResponse(response: Response): Promise<unknown> {
  const contentType = response.headers.get("content-type") ?? "";
  const body = await response.text();

  if (!body) {
    return null;
  }

  if (contentType.includes("application/json")) {
    return JSON.parse(body);
  }

  return body;
}

async function request<T>(
  config: ApiConfig,
  path: string,
  init: RequestInit = {}
): Promise<T> {
  const headers = new Headers(init.headers);
  const hasBody = init.body !== undefined;

  if (hasBody && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }

  if (config.token.trim().length > 0) {
    headers.set("authorization", `Bearer ${config.token.trim()}`);
  }

  const response = await fetch(`${trimBaseUrl(config.baseUrl)}${path}`, {
    ...init,
    headers
  });
  const payload = await parseResponse(response);

  if (!response.ok) {
    const message =
      typeof payload === "object" && payload !== null && "message" in payload
        ? String(payload.message)
        : `Request failed with ${response.status}`;
    throw new ApiError(message, response.status, payload);
  }

  return payload as T;
}

export const gatewayApi = {
  health: (config: ApiConfig) => request<Health>(config, "/healthz"),
  readiness: (config: ApiConfig) => request<{ status: "ok" }>(config, "/readyz"),
  adapters: (config: ApiConfig) => request<Adapter[]>(config, "/adapters"),
  tools: (config: ApiConfig) => request<Tool[]>(config, "/tools"),
  adapterStatus: (config: ApiConfig, name: string) =>
    request<DeploymentStatus>(config, `/adapters/${encodeURIComponent(name)}/status`),
  toolStatus: (config: ApiConfig, name: string) =>
    request<DeploymentStatus>(config, `/tools/${encodeURIComponent(name)}/status`),
  createAdapter: (config: ApiConfig, payload: CreateAdapterPayload) =>
    request<Adapter>(config, "/adapters", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  createTool: (config: ApiConfig, payload: CreateToolPayload) =>
    request<Tool>(config, "/tools", {
      method: "POST",
      body: JSON.stringify(payload)
    }),
  deleteAdapter: (config: ApiConfig, name: string) =>
    request<null>(config, `/adapters/${encodeURIComponent(name)}`, { method: "DELETE" }),
  deleteTool: (config: ApiConfig, name: string) =>
    request<null>(config, `/tools/${encodeURIComponent(name)}`, { method: "DELETE" }),
  invoke: (config: ApiConfig, target: string, payload: JsonRpcRequest) =>
    request<JsonRpcResponse>(config, target, {
      method: "POST",
      body: JSON.stringify(payload)
    })
};
