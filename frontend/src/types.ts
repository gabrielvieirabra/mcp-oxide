export type Theme = "light" | "dark";

export type Health = {
  status: "ok";
  version: string;
  providers?: Record<string, string>;
};

export type DeploymentStatus = {
  ready?: boolean;
  replicas?: number;
  ready_replicas?: number;
  message?: string;
};

export type EnvVar = {
  name: string;
  value: string;
};

export type SecretRef = {
  name: string;
  provider: string;
  key: string;
};

export type Resources = {
  cpu?: string;
  memory?: string;
};

export type HealthProbe = {
  path: string;
  port: number;
};

export type Adapter = {
  name: string;
  description?: string;
  image: string;
  endpoint_port: number;
  endpoint_path: string;
  upstream?: string;
  replicas: number;
  env: EnvVar[];
  secret_refs: SecretRef[];
  required_roles: string[];
  tags: string[];
  resources?: Resources;
  health?: HealthProbe;
  session_affinity: "sticky" | "none";
  labels: Record<string, string>;
  revision: number;
  created_at?: string;
  updated_at?: string;
};

export type ToolDefinition = {
  name: string;
  title?: string;
  description?: string;
  input_schema: Record<string, unknown>;
  annotations?: Record<string, unknown>;
};

export type Tool = {
  name: string;
  description?: string;
  image: string;
  endpoint_port: number;
  endpoint_path: string;
  tool_definition: ToolDefinition;
  env: EnvVar[];
  secret_refs: SecretRef[];
  required_roles: string[];
  tags: string[];
  resources?: Resources;
  revision: number;
  created_at?: string;
  updated_at?: string;
};

export type JsonRpcRequest = {
  jsonrpc: "2.0";
  id?: string | number | null;
  method: string;
  params?: unknown;
};

export type JsonRpcResponse = {
  jsonrpc: "2.0";
  id?: string | number | null;
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
};

export type CreateAdapterPayload = {
  name: string;
  description?: string;
  image: string;
  endpoint_port: number;
  endpoint_path: string;
  upstream?: string;
  replicas: number;
  required_roles: string[];
  tags: string[];
  resources?: Resources;
  session_affinity: "sticky" | "none";
  labels: Record<string, string>;
};

export type CreateToolPayload = {
  name: string;
  description?: string;
  image: string;
  endpoint_port: number;
  endpoint_path: string;
  tool_definition: ToolDefinition;
  required_roles: string[];
  tags: string[];
  resources?: Resources;
};
