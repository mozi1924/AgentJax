// Declarative provider plugin for OpenAI-compatible APIs.
//
// This plugin is purely declarative — it provides metadata (capabilities,
// config schema, model information, protocol mapping) used by the Rust
// framework. Protocol implementation (HTTP request building, SSE parsing)
// is handled natively in Rust by provider_api::protocol.
//
// Dependencies: agentjax SDK bootstrap provides withQuery, headerMap as globals.

// ── OpenAI Model Registry ───────────────────────────────────────────
const OPENAI_MODELS = {
  // GPT-5.5 / 5.0 series
  "gpt-5": { contextWindow: 400000 },
  "gpt-5.5": { contextWindow: 1000000 },
  "gpt-5-mini": { contextWindow: 400000 },
  // o3-mini
  "o3-mini": { contextWindow: 200000 },
  // o1 series
  "o1": { contextWindow: 128000 },
  "o1-mini": { contextWindow: 128000 },
  "o1-preview": { contextWindow: 128000 },
  // gpt-4o series
  "gpt-4o": { contextWindow: 128000 },
  "gpt-4o-mini": { contextWindow: 128000 },
  // legacy GPT-4 / GPT-3.5
  "gpt-4-turbo": { contextWindow: 128000 },
  "gpt-4": { contextWindow: 8192 },
  "gpt-4-32k": { contextWindow: 32768 },
  "gpt-3.5-turbo": { contextWindow: 16384 },
  // Embedding models
  "text-embedding-3-small": { contextWindow: 8191 },
  "text-embedding-3-large": { contextWindow: 8191 },
  "text-embedding-ada-002": { contextWindow: 8191 },
};

function resolveModelMetadata(modelId) {
  const normalized = (modelId || "").trim().toLowerCase();
  if (OPENAI_MODELS[normalized]) return OPENAI_MODELS[normalized];

  if (normalized.includes("gpt-5.5")) return OPENAI_MODELS["gpt-5.5"];
  if (normalized.includes("gpt-5-mini")) return OPENAI_MODELS["gpt-5-mini"];
  if (normalized.includes("gpt-5")) return OPENAI_MODELS["gpt-5"];
  if (normalized.startsWith("o3-mini")) return OPENAI_MODELS["o3-mini"];
  if (normalized.startsWith("o1")) return OPENAI_MODELS["o1"];
  if (normalized.includes("gpt-4o-mini")) return OPENAI_MODELS["gpt-4o-mini"];
  if (normalized.includes("gpt-4o")) return OPENAI_MODELS["gpt-4o"];
  if (normalized.includes("gpt-4-32k")) return { contextWindow: 32768 };
  if (normalized.includes("gpt-4-turbo")) return OPENAI_MODELS["gpt-4-turbo"];
  if (normalized.includes("gpt-4")) return OPENAI_MODELS["gpt-4"];
  if (normalized.includes("gpt-3.5")) return OPENAI_MODELS["gpt-3.5-turbo"];
  if (normalized.includes("text-embedding-3-small")) return OPENAI_MODELS["text-embedding-3-small"];
  if (normalized.includes("text-embedding-3-large")) return OPENAI_MODELS["text-embedding-3-large"];
  if (normalized.includes("text-embedding")) return OPENAI_MODELS["text-embedding-ada-002"];

  return { contextWindow: 128000 };
}

// ── Provider Definition (declarative only) ────────────────────────────

const openaiProvider = {
  kind: "openai",
  displayName: "OpenAI",
  defaultPriority: 0,
  defaultModelIds: ["gpt-5-mini", "gpt-5"],
  supportsProtocols: ["responses", "chat_completions", "embeddings"],
  toolSchemaFormat: "responses",
  configSchema: {
    type: "object",
    properties: {
      apiEndpoint: { type: "string", default: "https://api.openai.com/v1", title: "API Endpoint" },
      modelsEndpointCandidates: { type: "array", items: { type: "string" }, default: [] },
      queryParams: { type: "object", additionalProperties: { type: "string" }, default: {} },
      httpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      envHttpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      realtimeEndpoint: { type: ["string", "null"], default: null },
      supportsWebsockets: { type: "boolean", default: true },
      streamTransport: { type: "string", default: "sse" },
      credential: { type: ["string", "null"], default: null, sensitive: true },
      credentialEnv: { type: "string", default: "OPENAI_API_KEY" },
      requestTimeoutSeconds: { type: ["integer", "null"], default: null },
      requestMaxRetries: { type: ["integer", "null"], default: null },
      streamMaxRetries: { type: ["integer", "null"], default: null },
      streamIdleTimeoutMs: { type: ["integer", "null"], default: null },
      websocketConnectTimeoutMs: { type: ["integer", "null"], default: null },
    },
  },
  capabilities: {
    requiresInstructions: true,
    requiresStreamTrueInWebsocket: true,
    supportsStoredResponses: false,
    supportsCrossSocketContinuation: false,
    supportsGenerateFalse: true,
    supportsJsonMode: true,
    supportsJsonSchema: true,
    supportsParallelToolCalls: true,
    supportsBuiltInWebSearch: false,
    emitsFinalOutputItems: true,
    emitsIncrementalToolCallArguments: true,
  },

  // ── Model Protocol Mapping ───────────────────────────────────────
  // Tells the framework which protocol to use for a given model.
  // This is used by resolve_protocol() in the Rust registry.
  getModelProtocol(modelId) {
    const normalized = (modelId || "").trim().toLowerCase();
    if (normalized.includes("embedding") || normalized.startsWith("text-embedding-")) {
      return "embeddings";
    }
    // Chat Completions protocol for non-OpenAI models (e.g. custom endpoints)
    // that don't support the Responses API. Default to "responses" for
    // genuine OpenAI models.
    return "responses";
  },

  // ── Model Metadata ───────────────────────────────────────────────
  getModelMetadata({ modelId }) {
    return resolveModelMetadata(modelId);
  },

  // ── Reasoning Capability ─────────────────────────────────────────
  getReasoningCapability({ modelId, cachedLevels }) {
    const levels = cachedLevels && cachedLevels.length
      ? cachedLevels
      : (/^(gpt-5|o\d|o-|codex)/i.test(modelId) ? ["minimal", "low", "medium", "high"] : []);
    return { supportsReasoning: levels.length > 0, supportedReasoningLevels: levels };
  },

  // ── Remote Model Fetching ────────────────────────────────────────
  buildModelsRequest({ resolved }) {
    const candidates = resolved.provider.modelsEndpointCandidates || [];
    const url = candidates[0] || `${resolved.provider.apiEndpoint.replace(/\/+$/, "")}/models`;
    return {
      method: "GET",
      url: (typeof withQuery === "function" ? withQuery(url, resolved.provider.queryParams) : url),
      headers: (typeof headerMap === "function" ? headerMap({}, resolved) : {}),
      authStrategy: "bearer",
    };
  },

  parseModelsResponse({ response }) {
    return (response.data || response.models || []).map((model) => ({
      id: model.id || model.name || String(model),
      supportedReasoningLevels: model.supported_reasoning_levels || model.supportedReasoningLevels || [],
    })).filter((model) => model.id);
  },
};

globalThis.AgentJaxPlugin = {
  providers: [openaiProvider],
  tools: {},
};
