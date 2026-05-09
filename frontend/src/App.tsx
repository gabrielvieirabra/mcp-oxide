import {
  Activity,
  Boxes,
  Braces,
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Copy,
  Database,
  Moon,
  PackagePlus,
  Play,
  PlugZap,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  Sun,
  Trash2,
  Wrench
} from "lucide-react";
import { FormEvent, ReactNode, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { gatewayApi, type ApiConfig, type ApiError } from "./api";
import type {
  Adapter,
  CreateAdapterPayload,
  CreateToolPayload,
  DeploymentStatus,
  Health,
  JsonRpcRequest,
  JsonRpcResponse,
  Theme,
  Tool
} from "./types";
import { compactRecord, splitList, useLocalStorageState } from "./storage";

type View = "overview" | "adapters" | "tools" | "playground";

const navItems: Array<{ id: View; label: string; icon: ReactNode }> = [
  { id: "overview", label: "Overview", icon: <Activity size={18} /> },
  { id: "adapters", label: "Adapters", icon: <Server size={18} /> },
  { id: "tools", label: "Tools", icon: <Wrench size={18} /> },
  { id: "playground", label: "Playground", icon: <Braces size={18} /> }
];

const defaultSchema = JSON.stringify(
  {
    type: "object",
    properties: {},
    additionalProperties: false
  },
  null,
  2
);

function App() {
  const [view, setView] = useState<View>("overview");
  const [theme, setTheme] = useLocalStorageState<Theme>("mcp-oxide-theme", "dark");
  const [baseUrl, setBaseUrl] = useLocalStorageState("mcp-oxide-base-url", "");
  const [token, setToken] = useLocalStorageState("mcp-oxide-token", "");

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  const apiConfig = useMemo<ApiConfig>(
    () => ({
      baseUrl: baseUrl.trim() || window.location.origin,
      token
    }),
    [baseUrl, token]
  );

  const hasToken = token.trim().length > 0;
  const healthQuery = useQuery({
    queryKey: ["health", apiConfig.baseUrl, token],
    queryFn: () => gatewayApi.health(apiConfig),
    refetchInterval: 15_000
  });
  const readyQuery = useQuery({
    queryKey: ["ready", apiConfig.baseUrl, token],
    queryFn: () => gatewayApi.readiness(apiConfig),
    refetchInterval: 15_000
  });
  const adaptersQuery = useQuery({
    queryKey: ["adapters", apiConfig.baseUrl, token],
    queryFn: () => gatewayApi.adapters(apiConfig),
    enabled: hasToken
  });
  const toolsQuery = useQuery({
    queryKey: ["tools", apiConfig.baseUrl, token],
    queryFn: () => gatewayApi.tools(apiConfig),
    enabled: hasToken
  });

  const adapters = adaptersQuery.data ?? [];
  const tools = toolsQuery.data ?? [];

  const refreshAll = () => {
    void healthQuery.refetch();
    void readyQuery.refetch();
    if (hasToken) {
      void adaptersQuery.refetch();
      void toolsQuery.refetch();
    }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <PlugZap size={22} />
          </div>
          <div>
            <strong>mcp-oxide</strong>
            <span>Console</span>
          </div>
        </div>

        <nav className="nav-list" aria-label="Primary">
          {navItems.map((item) => (
            <button
              key={item.id}
              className={view === item.id ? "nav-item active" : "nav-item"}
              onClick={() => setView(item.id)}
              type="button"
            >
              {item.icon}
              <span>{item.label}</span>
              <ChevronRight size={16} />
            </button>
          ))}
        </nav>

        <div className="sidebar-status">
          <StatusPill
            tone={healthQuery.data?.status === "ok" ? "good" : healthQuery.isError ? "bad" : "muted"}
            label={healthQuery.data?.status === "ok" ? "Gateway online" : "Gateway unknown"}
          />
          <small>{apiConfig.baseUrl}</small>
        </div>
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div className="connection-panel">
            <label>
              <span>Gateway</span>
              <input
                value={baseUrl}
                onChange={(event) => setBaseUrl(event.target.value)}
                placeholder={window.location.origin}
                spellCheck={false}
              />
            </label>
            <label>
              <span>Bearer token</span>
              <input
                value={token}
                onChange={(event) => setToken(event.target.value)}
                placeholder="eyJ..."
                type="password"
                spellCheck={false}
              />
            </label>
          </div>

          <div className="topbar-actions">
            <button className="icon-button" onClick={refreshAll} title="Refresh" type="button">
              <RefreshCw size={18} />
            </button>
            <button
              className="icon-button"
              onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
              title="Toggle theme"
              type="button"
            >
              {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
            </button>
          </div>
        </header>

        {view === "overview" && (
          <Overview
            adapters={adapters}
            adaptersError={errorMessage(adaptersQuery.error)}
            health={healthQuery.data}
            healthError={errorMessage(healthQuery.error)}
            isLoading={healthQuery.isLoading}
            ready={readyQuery.data?.status === "ok"}
            tools={tools}
            toolsError={errorMessage(toolsQuery.error)}
            tokenPresent={hasToken}
          />
        )}
        {view === "adapters" && (
          <ResourcesView
            apiConfig={apiConfig}
            adapters={adapters}
            isLoading={adaptersQuery.isFetching}
            kind="adapter"
            queryError={errorMessage(adaptersQuery.error)}
            tokenPresent={hasToken}
          />
        )}
        {view === "tools" && (
          <ResourcesView
            apiConfig={apiConfig}
            isLoading={toolsQuery.isFetching}
            kind="tool"
            queryError={errorMessage(toolsQuery.error)}
            tokenPresent={hasToken}
            tools={tools}
          />
        )}
        {view === "playground" && (
          <Playground apiConfig={apiConfig} adapters={adapters} tokenPresent={hasToken} />
        )}
      </main>
    </div>
  );
}

function Overview({
  adapters,
  adaptersError,
  health,
  healthError,
  isLoading,
  ready,
  tokenPresent,
  tools,
  toolsError
}: {
  adapters: Adapter[];
  adaptersError?: string;
  health?: Health;
  healthError?: string;
  isLoading: boolean;
  ready: boolean;
  tokenPresent: boolean;
  tools: Tool[];
  toolsError?: string;
}) {
  const providerEntries = Object.entries(health?.providers ?? {});

  return (
    <section className="view-stack">
      <div className="view-heading">
        <div>
          <span className="eyebrow">Control plane</span>
          <h1>Operations overview</h1>
        </div>
        <StatusPill
          tone={health?.status === "ok" ? "good" : healthError ? "bad" : "muted"}
          label={isLoading ? "Checking" : health?.status === "ok" ? "Healthy" : "Unavailable"}
        />
      </div>

      <div className="metric-grid">
        <MetricCard icon={<ShieldCheck size={20} />} label="Gateway" value={health?.version ?? "--"} meta={healthError ?? "version"} />
        <MetricCard icon={<Database size={20} />} label="Readiness" value={ready ? "Ready" : "Pending"} meta="/readyz" />
        <MetricCard icon={<Server size={20} />} label="Adapters" value={tokenPresent ? adapters.length : "--"} meta={adaptersError ?? "registered"} />
        <MetricCard icon={<Wrench size={20} />} label="Tools" value={tokenPresent ? tools.length : "--"} meta={toolsError ?? "published"} />
      </div>

      <div className="split-grid">
        <section className="panel">
          <div className="panel-heading">
            <h2>Providers</h2>
            <span>{providerEntries.length}</span>
          </div>
          <div className="provider-grid">
            {providerEntries.map(([name, value]) => (
              <div className="provider-row" key={name}>
                <span>{name}</span>
                <strong>{value}</strong>
              </div>
            ))}
            {providerEntries.length === 0 && <EmptyState icon={<Boxes />} title="No provider summary" />}
          </div>
        </section>

        <section className="panel">
          <div className="panel-heading">
            <h2>Recent inventory</h2>
            <span>{adapters.length + tools.length}</span>
          </div>
          <div className="inventory-list">
            {[...adapters.slice(0, 4), ...tools.slice(0, 4)].slice(0, 6).map((item) => (
              <div className="inventory-row" key={`${"tool_definition" in item ? "tool" : "adapter"}-${item.name}`}>
                <div>
                  <strong>{item.name}</strong>
                  <span>{item.image}</span>
                </div>
                <StatusPill tone="muted" label={"tool_definition" in item ? "tool" : "adapter"} />
              </div>
            ))}
            {!tokenPresent && <EmptyState icon={<ShieldCheck />} title="Token required" />}
            {tokenPresent && adapters.length + tools.length === 0 && <EmptyState icon={<Search />} title="No resources" />}
          </div>
        </section>
      </div>
    </section>
  );
}

function ResourcesView(props: {
  apiConfig: ApiConfig;
  adapters?: Adapter[];
  isLoading: boolean;
  kind: "adapter" | "tool";
  queryError?: string;
  tokenPresent: boolean;
  tools?: Tool[];
}) {
  const resources = useMemo(
    () => (props.kind === "adapter" ? props.adapters ?? [] : props.tools ?? []),
    [props.adapters, props.kind, props.tools]
  );
  const [requestedSelectedName, setRequestedSelectedName] = useState("");
  const selected = resources.find((resource) => resource.name === requestedSelectedName) ?? resources[0];
  const selectedName = selected?.name ?? "";

  return (
    <section className="view-stack">
      <div className="view-heading">
        <div>
          <span className="eyebrow">{props.kind === "adapter" ? "MCP servers" : "Tool router"}</span>
          <h1>{props.kind === "adapter" ? "Adapters" : "Tools"}</h1>
        </div>
        <StatusPill tone={props.queryError ? "bad" : "muted"} label={props.queryError ?? `${resources.length} total`} />
      </div>

      <div className="management-grid">
        <section className="panel">
          <div className="panel-heading">
            <h2>Inventory</h2>
            {props.isLoading && <span>Syncing</span>}
          </div>
          <ResourceTable
            apiConfig={props.apiConfig}
            kind={props.kind}
            onSelect={setRequestedSelectedName}
            resources={resources}
            selectedName={selectedName}
            tokenPresent={props.tokenPresent}
          />
        </section>

        <ResourceDetail apiConfig={props.apiConfig} kind={props.kind} resource={selected} />
      </div>

      <ResourceCreator apiConfig={props.apiConfig} kind={props.kind} tokenPresent={props.tokenPresent} />
    </section>
  );
}

function ResourceTable({
  apiConfig,
  kind,
  onSelect,
  resources,
  selectedName,
  tokenPresent
}: {
  apiConfig: ApiConfig;
  kind: "adapter" | "tool";
  onSelect: (name: string) => void;
  resources: Array<Adapter | Tool>;
  selectedName: string;
  tokenPresent: boolean;
}) {
  const queryClient = useQueryClient();
  const deleteMutation = useMutation({
    mutationFn: (name: string) =>
      kind === "adapter" ? gatewayApi.deleteAdapter(apiConfig, name) : gatewayApi.deleteTool(apiConfig, name),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: [kind === "adapter" ? "adapters" : "tools"] });
    }
  });

  if (!tokenPresent) {
    return <EmptyState icon={<ShieldCheck />} title="Token required" />;
  }

  if (resources.length === 0) {
    return <EmptyState icon={kind === "adapter" ? <Server /> : <Wrench />} title="No resources" />;
  }

  return (
    <div className="resource-table" role="table">
      <div className="table-head" role="row">
        <span>Name</span>
        <span>Image</span>
        <span>Tags</span>
        <span />
      </div>
      {resources.map((resource) => (
        <div className={resource.name === selectedName ? "table-row selected" : "table-row"} key={resource.name} role="row">
          <button className="row-main" onClick={() => onSelect(resource.name)} type="button">
            <span>
              <strong>{resource.name}</strong>
              <small>rev {resource.revision}</small>
            </span>
            <span className="image-cell">{resource.image}</span>
            <span className="tag-list">
              {resource.tags.slice(0, 3).map((tag) => (
                <b key={tag}>{tag}</b>
              ))}
            </span>
          </button>
          <span className="row-actions">
            <button
              className="icon-button danger"
              disabled={deleteMutation.isPending}
              onClick={() => deleteMutation.mutate(resource.name)}
              title="Delete"
              type="button"
            >
              <Trash2 size={16} />
            </button>
          </span>
        </div>
      ))}
      {deleteMutation.error && <p className="error-text">{errorMessage(deleteMutation.error)}</p>}
    </div>
  );
}

function ResourceDetail({
  apiConfig,
  kind,
  resource
}: {
  apiConfig: ApiConfig;
  kind: "adapter" | "tool";
  resource?: Adapter | Tool;
}) {
  const statusQuery = useQuery({
    queryKey: [kind, "status", resource?.name, apiConfig.baseUrl, apiConfig.token],
    queryFn: () =>
      kind === "adapter"
        ? gatewayApi.adapterStatus(apiConfig, resource?.name ?? "")
        : gatewayApi.toolStatus(apiConfig, resource?.name ?? ""),
    enabled: Boolean(resource)
  });

  if (!resource) {
    return (
      <section className="panel detail-panel">
        <EmptyState icon={<Search />} title="Nothing selected" />
      </section>
    );
  }

  const status = statusQuery.data;

  return (
    <section className="panel detail-panel">
      <div className="panel-heading">
        <h2>{resource.name}</h2>
        <StatusPill tone={status?.ready ? "good" : statusQuery.error ? "bad" : "muted"} label={statusLabel(status, statusQuery.error)} />
      </div>

      <dl className="detail-grid">
        <div>
          <dt>Image</dt>
          <dd>{resource.image}</dd>
        </div>
        <div>
          <dt>Endpoint</dt>
          <dd>
            {resource.endpoint_port}
            {resource.endpoint_path}
          </dd>
        </div>
        {"replicas" in resource && (
          <div>
            <dt>Replicas</dt>
            <dd>
              {status?.ready_replicas ?? 0}/{status?.replicas ?? resource.replicas}
            </dd>
          </div>
        )}
        <div>
          <dt>Roles</dt>
          <dd>{resource.required_roles.join(", ") || "none"}</dd>
        </div>
      </dl>

      <div className="tag-list large">
        {resource.tags.map((tag) => (
          <b key={tag}>{tag}</b>
        ))}
      </div>

      <pre className="json-block">{JSON.stringify(resource, null, 2)}</pre>
    </section>
  );
}

function ResourceCreator({
  apiConfig,
  kind,
  tokenPresent
}: {
  apiConfig: ApiConfig;
  kind: "adapter" | "tool";
  tokenPresent: boolean;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [image, setImage] = useState("");
  const [endpointPort, setEndpointPort] = useState(8080);
  const [endpointPath, setEndpointPath] = useState("/mcp");
  const [upstream, setUpstream] = useState("");
  const [replicas, setReplicas] = useState(1);
  const [roles, setRoles] = useState("mcp.admin");
  const [tags, setTags] = useState("");
  const [cpu, setCpu] = useState("");
  const [memory, setMemory] = useState("");
  const [sessionAffinity, setSessionAffinity] = useState<"sticky" | "none">("sticky");
  const [toolTitle, setToolTitle] = useState("");
  const [toolSchema, setToolSchema] = useState(defaultSchema);
  const [formError, setFormError] = useState("");

  const createMutation = useMutation<Adapter | Tool, Error, CreateAdapterPayload | CreateToolPayload>({
    mutationFn: (payload: CreateAdapterPayload | CreateToolPayload) =>
      kind === "adapter"
        ? gatewayApi.createAdapter(apiConfig, payload as CreateAdapterPayload)
        : gatewayApi.createTool(apiConfig, payload as CreateToolPayload),
    onSuccess: async () => {
      setName("");
      setDescription("");
      setImage("");
      setTags("");
      setFormError("");
      await queryClient.invalidateQueries({ queryKey: [kind === "adapter" ? "adapters" : "tools"] });
    }
  });

  const submit = (event: FormEvent) => {
    event.preventDefault();
    setFormError("");

    const resources = compactRecord({ cpu, memory });
    const base = {
      name,
      description: description || undefined,
      image,
      endpoint_port: endpointPort,
      endpoint_path: endpointPath,
      required_roles: splitList(roles),
      tags: splitList(tags),
      resources: Object.keys(resources).length > 0 ? resources : undefined
    };

    if (kind === "adapter") {
      createMutation.mutate({
        ...base,
        upstream: upstream || undefined,
        replicas,
        session_affinity: sessionAffinity,
        labels: {}
      });
      return;
    }

    try {
      const inputSchema = JSON.parse(toolSchema) as Record<string, unknown>;
      createMutation.mutate({
        ...base,
        tool_definition: {
          name,
          title: toolTitle || undefined,
          description: description || undefined,
          input_schema: inputSchema
        }
      });
    } catch {
      setFormError("Invalid JSON schema");
    }
  };

  return (
    <section className="panel creator-panel">
      <div className="panel-heading">
        <h2>{kind === "adapter" ? "Create adapter" : "Create tool"}</h2>
        <PackagePlus size={18} />
      </div>

      <form className="creator-form" onSubmit={submit}>
        <Field label="Name">
          <input required value={name} onChange={(event) => setName(event.target.value)} placeholder="echo" />
        </Field>
        <Field label="Image">
          <input required value={image} onChange={(event) => setImage(event.target.value)} placeholder="ghcr.io/acme/echo:latest" />
        </Field>
        <Field label="Description">
          <input value={description} onChange={(event) => setDescription(event.target.value)} placeholder="Short label" />
        </Field>
        <Field label="Port">
          <input min={1} max={65535} type="number" value={endpointPort} onChange={(event) => setEndpointPort(Number(event.target.value))} />
        </Field>
        <Field label="Path">
          <input value={endpointPath} onChange={(event) => setEndpointPath(event.target.value)} />
        </Field>
        {kind === "adapter" && (
          <>
            <Field label="Upstream">
              <input value={upstream} onChange={(event) => setUpstream(event.target.value)} placeholder="http://service:8080" />
            </Field>
            <Field label="Replicas">
              <input min={0} type="number" value={replicas} onChange={(event) => setReplicas(Number(event.target.value))} />
            </Field>
            <Field label="Affinity">
              <select value={sessionAffinity} onChange={(event) => setSessionAffinity(event.target.value as "sticky" | "none")}>
                <option value="sticky">sticky</option>
                <option value="none">none</option>
              </select>
            </Field>
          </>
        )}
        {kind === "tool" && (
          <>
            <Field label="Title">
              <input value={toolTitle} onChange={(event) => setToolTitle(event.target.value)} placeholder="Echo tool" />
            </Field>
            <Field label="Input schema" wide>
              <textarea value={toolSchema} onChange={(event) => setToolSchema(event.target.value)} rows={7} spellCheck={false} />
            </Field>
          </>
        )}
        <Field label="Roles">
          <input value={roles} onChange={(event) => setRoles(event.target.value)} placeholder="mcp.admin,mcp.viewer" />
        </Field>
        <Field label="Tags">
          <input value={tags} onChange={(event) => setTags(event.target.value)} placeholder="prod,search" />
        </Field>
        <Field label="CPU">
          <input value={cpu} onChange={(event) => setCpu(event.target.value)} placeholder="500m" />
        </Field>
        <Field label="Memory">
          <input value={memory} onChange={(event) => setMemory(event.target.value)} placeholder="512Mi" />
        </Field>

        <div className="form-footer">
          <button className="primary-button" disabled={!tokenPresent || createMutation.isPending} type="submit">
            <PackagePlus size={17} />
            <span>{createMutation.isPending ? "Creating" : "Create"}</span>
          </button>
          {(formError || createMutation.error) && <p className="error-text">{formError || errorMessage(createMutation.error)}</p>}
        </div>
      </form>
    </section>
  );
}

function Playground({
  adapters,
  apiConfig,
  tokenPresent
}: {
  adapters: Adapter[];
  apiConfig: ApiConfig;
  tokenPresent: boolean;
}) {
  const [target, setTarget] = useState("router");
  const [method, setMethod] = useState("tools/list");
  const [id, setId] = useState("1");
  const [params, setParams] = useState("{}");
  const [response, setResponse] = useState<JsonRpcResponse | null>(null);
  const [localError, setLocalError] = useState("");

  const invokeMutation = useMutation({
    mutationFn: (payload: { path: string; request: JsonRpcRequest }) =>
      gatewayApi.invoke(apiConfig, payload.path, payload.request),
    onSuccess: (data) => {
      setResponse(data);
      setLocalError("");
    }
  });

  const submit = (event: FormEvent) => {
    event.preventDefault();
    setLocalError("");

    try {
      const parsedParams = params.trim() ? JSON.parse(params) : undefined;
      const request: JsonRpcRequest = {
        jsonrpc: "2.0",
        id: id.trim() || undefined,
        method,
        params: parsedParams
      };
      const path = target === "router" ? "/mcp" : `/adapters/${encodeURIComponent(target)}/mcp`;
      invokeMutation.mutate({ path, request });
    } catch {
      setLocalError("Invalid params JSON");
    }
  };

  return (
    <section className="view-stack">
      <div className="view-heading">
        <div>
          <span className="eyebrow">Data plane</span>
          <h1>JSON-RPC playground</h1>
        </div>
        <StatusPill tone={tokenPresent ? "good" : "muted"} label={tokenPresent ? "Token loaded" : "Token required"} />
      </div>

      <div className="playground-grid">
        <section className="panel">
          <form className="rpc-form" onSubmit={submit}>
            <Field label="Target">
              <select value={target} onChange={(event) => setTarget(event.target.value)}>
                <option value="router">/mcp</option>
                {adapters.map((adapter) => (
                  <option key={adapter.name} value={adapter.name}>
                    /adapters/{adapter.name}/mcp
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Method">
              <input value={method} onChange={(event) => setMethod(event.target.value)} />
            </Field>
            <Field label="ID">
              <input value={id} onChange={(event) => setId(event.target.value)} />
            </Field>
            <Field label="Params" wide>
              <textarea value={params} onChange={(event) => setParams(event.target.value)} rows={12} spellCheck={false} />
            </Field>
            <button className="primary-button" disabled={!tokenPresent || invokeMutation.isPending} type="submit">
              <Play size={17} />
              <span>{invokeMutation.isPending ? "Running" : "Invoke"}</span>
            </button>
            {(localError || invokeMutation.error) && <p className="error-text">{localError || errorMessage(invokeMutation.error)}</p>}
          </form>
        </section>

        <section className="panel response-panel">
          <div className="panel-heading">
            <h2>Response</h2>
            <button
              className="icon-button"
              disabled={!response}
              onClick={() => response && navigator.clipboard.writeText(JSON.stringify(response, null, 2))}
              title="Copy"
              type="button"
            >
              <Copy size={16} />
            </button>
          </div>
          <pre className="json-block tall">{response ? JSON.stringify(response, null, 2) : "{}"}</pre>
        </section>
      </div>
    </section>
  );
}

function Field({ children, label, wide = false }: { children: ReactNode; label: string; wide?: boolean }) {
  return (
    <label className={wide ? "field wide" : "field"}>
      <span>{label}</span>
      {children}
    </label>
  );
}

function MetricCard({ icon, label, meta, value }: { icon: ReactNode; label: string; meta: string; value: ReactNode }) {
  return (
    <section className="metric-card">
      <div className="metric-icon">{icon}</div>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{meta}</small>
    </section>
  );
}

function StatusPill({ label, tone }: { label: string; tone: "good" | "bad" | "muted" }) {
  const Icon = tone === "good" ? CheckCircle2 : tone === "bad" ? CircleAlert : Activity;

  return (
    <span className={`status-pill ${tone}`}>
      <Icon size={15} />
      {label}
    </span>
  );
}

function EmptyState({ icon, title }: { icon: ReactNode; title: string }) {
  return (
    <div className="empty-state">
      {icon}
      <strong>{title}</strong>
    </div>
  );
}

function statusLabel(status?: DeploymentStatus, error?: Error | null) {
  if (error) {
    return "Status unavailable";
  }

  if (!status) {
    return "Checking";
  }

  if (status.ready) {
    return "Ready";
  }

  return status.message ?? "Pending";
}

function errorMessage(error: unknown) {
  if (!error) {
    return undefined;
  }

  const apiError = error as Partial<ApiError>;
  if (typeof apiError.message === "string") {
    return apiError.message;
  }

  return "Request failed";
}

export default App;
