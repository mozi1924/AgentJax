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
  "gpt-5": { contextWindow: 400000, kind: "chat" },
  "gpt-5.5": { contextWindow: 1000000, kind: "chat" },
  "gpt-5-mini": { contextWindow: 400000, kind: "chat" },
  // o3-mini
  "o3-mini": { contextWindow: 200000, kind: "chat" },
  // o1 series
  "o1": { contextWindow: 128000, kind: "chat" },
  "o1-mini": { contextWindow: 128000, kind: "chat" },
  "o1-preview": { contextWindow: 128000, kind: "chat" },
  // gpt-4o series
  "gpt-4o": { contextWindow: 128000, kind: "chat" },
  "gpt-4o-mini": { contextWindow: 128000, kind: "chat" },
  // legacy GPT-4 / GPT-3.5
  "gpt-4-turbo": { contextWindow: 128000, kind: "chat" },
  "gpt-4": { contextWindow: 8192, kind: "chat" },
  "gpt-4-32k": { contextWindow: 32768, kind: "chat" },
  "gpt-3.5-turbo": { contextWindow: 16384, kind: "chat" },
  // Embedding models
  "text-embedding-3-small": { contextWindow: 8191, kind: "embedding" },
  "text-embedding-3-large": { contextWindow: 8191, kind: "embedding" },
  "text-embedding-ada-002": { contextWindow: 8191, kind: "embedding" },
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
  if (normalized.includes("gpt-4-32k")) return { contextWindow: 32768, kind: "chat" };
  if (normalized.includes("gpt-4-turbo")) return OPENAI_MODELS["gpt-4-turbo"];
  if (normalized.includes("gpt-4")) return OPENAI_MODELS["gpt-4"];
  if (normalized.includes("gpt-3.5")) return OPENAI_MODELS["gpt-3.5-turbo"];
  if (normalized.includes("text-embedding-3-small")) return OPENAI_MODELS["text-embedding-3-small"];
  if (normalized.includes("text-embedding-3-large")) return OPENAI_MODELS["text-embedding-3-large"];
  if (normalized.includes("text-embedding")) return OPENAI_MODELS["text-embedding-ada-002"];
  if (normalized.includes("embedding")) return { contextWindow: 8191, kind: "embedding" };

  return { contextWindow: 128000, kind: "chat" };
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
  //
  // Resolution strategy:
  //   1. Embedding models → "embeddings"
  //   2. Known OpenAI first-party models → "responses" (native API)
  //   3. Unknown / third-party / self-hosted models → "chat_completions"
  //      (most open-source and compatible APIs use Chat Completions)
  //
  // Users can override this per-model via the apiProtocol config field.
  getModelProtocol(modelId) {
    const normalized = (modelId || "").trim().toLowerCase();
    if (normalized.includes("embedding") || normalized.startsWith("text-embedding-")) {
      return "embeddings";
    }

    // Known OpenAI first-party model prefixes that support the Responses API.
    const openaiNativePrefixes = [
      "gpt-5", "gpt-4", "gpt-3.5",
      "o1", "o3", "o4",
    ];
    const isOpenaiNative = openaiNativePrefixes.some((prefix) => normalized.startsWith(prefix));
    if (isOpenaiNative) {
      return "responses";
    }

    // Default to Chat Completions for all other models:
    // DeepSeek, Ollama, vLLM, LM Studio, OpenRouter, custom fine-tunes, etc.
    return "chat_completions";
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
