// Built-in provider plugin for Gemini APIs.
// Dependencies: agentjax SDK bootstrap provides withQuery, event, textFromContent,
// parseArgs, headerMap, usageFrom as globals.
// The plugin owns provider-specific request conversion, stream parsing, etc.

function geminiContents(items) {
  return (items || []).map((item) => {
    if (item.type === "function_call") {
      return { role: "model", parts: [{ functionCall: { name: item.name, args: parseArgs(item.arguments) } }] };
    }
    if (item.type === "function_call_output") {
      let toolName = item.name;
      if (!toolName && item.call_id) {
        const matchingCall = items.find((x) => x.type === "function_call" && x.call_id === item.call_id);
        if (matchingCall) {
          toolName = matchingCall.name;
        }
      }
      return { role: "user", parts: [{ functionResponse: { name: toolName || "tool", response: { result: item.output || "" } } }] };
    }
    const role = item.role === "assistant" ? "model" : "user";
    const text = textFromContent(item.content);
    return text.trim() ? { role, parts: [{ text }] } : null;
  }).filter(Boolean);
}

function initialState(state) {
  return { responseId: "", outputText: "", outputItems: [], usage: null, emittedOutputStarted: false, nextTool: 0, ...(state || {}) };
}

// ── Gemini Model Registry ───────────────────────────────────────────
const GEMINI_MODELS = {
  // Gemini 3.5 series
  "gemini-3.5-pro": { contextWindow: 2000000 },
  "gemini-3.5-flash": { contextWindow: 1000000 },
  // Gemini 3.0 series
  "gemini-3.0-pro": { contextWindow: 2000000 },
  "gemini-3.0-flash": { contextWindow: 1000000 },
  "gemini-3-pro": { contextWindow: 2000000 },
  "gemini-3-flash": { contextWindow: 1000000 },
  // Gemini 2.5 series
  "gemini-2.5-pro": { contextWindow: 2000000 },
  "gemini-2.5-flash": { contextWindow: 1000000 },
  // Gemini 2.0 series
  "gemini-2.0-pro": { contextWindow: 2000000 },
  "gemini-2.0-flash": { contextWindow: 1000000 },
  "gemini-2.0-flash-lite": { contextWindow: 1000000 },
  // Gemini 1.5 series
  "gemini-1.5-pro": { contextWindow: 2000000 },
  "gemini-1.5-flash": { contextWindow: 1000000 },
};

function resolveGeminiModelMetadata(modelId) {
  const normalized = (modelId || "").trim().toLowerCase().replace(/^models\//, "");
  if (GEMINI_MODELS[normalized]) return GEMINI_MODELS[normalized];

  if (normalized.includes("gemini-3.5-pro")) return GEMINI_MODELS["gemini-3.5-pro"];
  if (normalized.includes("gemini-3.5-flash")) return GEMINI_MODELS["gemini-3.5-flash"];
  if (normalized.includes("gemini-3.5")) return GEMINI_MODELS["gemini-3.5-pro"];

  if (normalized.includes("gemini-3-pro") || normalized.includes("gemini-3.0-pro") || normalized.includes("gemini-3.1-pro")) return GEMINI_MODELS["gemini-3-pro"];
  if (normalized.includes("gemini-3-flash") || normalized.includes("gemini-3.0-flash")) return GEMINI_MODELS["gemini-3-flash"];
  if (normalized.includes("gemini-3")) return GEMINI_MODELS["gemini-3-pro"];

  if (normalized.includes("gemini-2.5-pro")) return GEMINI_MODELS["gemini-2.5-pro"];
  if (normalized.includes("gemini-2.5-flash")) return GEMINI_MODELS["gemini-2.5-flash"];
  if (normalized.includes("gemini-2.5")) return GEMINI_MODELS["gemini-2.5-pro"];

  if (normalized.includes("gemini-2.0-pro")) return GEMINI_MODELS["gemini-2.0-pro"];
  if (normalized.includes("gemini-2.0-flash-lite")) return GEMINI_MODELS["gemini-2.0-flash-lite"];
  if (normalized.includes("gemini-2.0-flash")) return GEMINI_MODELS["gemini-2.0-flash"];
  if (normalized.includes("gemini-2.0")) return GEMINI_MODELS["gemini-2.0-flash"];

  if (normalized.includes("gemini-1.5-pro")) return GEMINI_MODELS["gemini-1.5-pro"];
  if (normalized.includes("gemini-1.5-flash")) return GEMINI_MODELS["gemini-1.5-flash"];
  if (normalized.includes("gemini-1.5")) return GEMINI_MODELS["gemini-1.5-flash"];

  if (normalized.includes("flash")) return GEMINI_MODELS["gemini-3.5-flash"];
  if (normalized.includes("pro")) return GEMINI_MODELS["gemini-3.0-pro"];

  return { contextWindow: 1000000 };
}

const geminiProvider = {
  kind: "gemini",
  displayName: "Gemini",
  defaultPriority: 40,
  defaultModelIds: ["gemini-2.5-flash", "gemini-2.5-pro"],
  toolSchemaFormat: "gemini",
  configSchema: {
    type: "object",
    properties: {
      apiEndpoint: { type: "string", default: "https://generativelanguage.googleapis.com/v1beta", title: "API Endpoint" },
      modelsEndpointCandidates: { type: "array", items: { type: "string" }, default: [] },
      queryParams: { type: "object", additionalProperties: { type: "string" }, default: {} },
      httpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      envHttpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      realtimeEndpoint: { type: ["string", "null"], default: null },
      supportsWebsockets: { type: "boolean", default: false },
      streamTransport: { type: "string", default: "sse" },
      credential: { type: ["string", "null"], default: null, sensitive: true },
      credentialEnv: { type: "string", default: "GEMINI_API_KEY" },
      requestTimeoutSeconds: { type: ["integer", "null"], default: null },
      requestMaxRetries: { type: ["integer", "null"], default: null },
      streamMaxRetries: { type: ["integer", "null"], default: null },
      streamIdleTimeoutMs: { type: ["integer", "null"], default: null },
      websocketConnectTimeoutMs: { type: ["integer", "null"], default: null },
    },
  },
  capabilities: {
    requiresInstructions: false,
    requiresStreamTrueInWebsocket: false,
    supportsStoredResponses: false,
    supportsCrossSocketContinuation: false,
    supportsGenerateFalse: false,
    supportsJsonMode: true,
    supportsJsonSchema: true,
    supportsParallelToolCalls: true,
    supportsBuiltInWebSearch: false,
    emitsFinalOutputItems: false,
    emitsIncrementalToolCallArguments: false,
  },
  buildStreamRequest({ resolved, request }) {
    const base = resolved.provider.apiEndpoint.replace(/\/+$/, "");
    const model = resolved.modelId.replace(/^models\//, "");
    const useKeyQuery = base.includes("generativelanguage.googleapis.com");
    const query = { ...(resolved.provider.queryParams || {}), alt: "sse" };
    // ⚠️  Credential is NOT added here — Rust injects it server-side
    // after the plugin returns, based on authStrategy.
    const body = { contents: geminiContents(request.inputItems) };
    const instructions = (request.instructionsOverride || "").trim() || resolved.systemPrompt;
    if (instructions.trim()) body.systemInstruction = { parts: [{ text: instructions }] };
    if (request.tools && request.tools.length) body.tools = [{ functionDeclarations: request.tools }];
    const generationConfig = {};
    const cfg = resolved.requestConfig || {};
    if (cfg.temperature != null) generationConfig.temperature = cfg.temperature;
    if (cfg.topP != null) generationConfig.topP = cfg.topP;
    if (cfg.topK != null) generationConfig.topK = cfg.topK;
    if (cfg.maxOutputTokens != null) generationConfig.maxOutputTokens = cfg.maxOutputTokens;
    if (Object.keys(generationConfig).length) body.generationConfig = generationConfig;
    Object.assign(body, cfg.extraBody || {});
    return {
      method: "POST",
      url: withQuery(`${base}/models/${model}:streamGenerateContent`, query),
      streamProtocol: "sse",
      headers: headerMap({ "Content-Type": "application/json", Accept: "text/event-stream" }, resolved),
      body,
      authStrategy: useKeyQuery ? "key-query" : "bearer",
    };
  },
  parseStreamEvent({ state, eventBlock }) {
    const next = initialState(state);
    const data = eventBlock.split(/\r?\n/).filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trimStart()).join("\n");
    if (!data || data === "[DONE]") return { state: next, done: data === "[DONE]" };
    const value = JSON.parse(data);
    if (value.error) throw new Error(`Gemini streaming error: ${JSON.stringify(value.error)}`);
    const usage = usageFrom(value);
    if (usage) next.usage = usage;
    const events = [];
    for (const candidate of value.candidates || []) {
      for (const part of candidate.content?.parts || []) {
        if (part.text) {
          if (!next.emittedOutputStarted) {
            next.emittedOutputStarted = true;
            events.push(event("OutputTextStarted"));
          }
          next.outputText += part.text;
          events.push(event("OutputTextDelta", { delta: part.text, phase: null }));
        }
        if (part.functionCall) {
          const name = part.functionCall.name || "tool";
          const index = next.nextTool++;
          const item_id = `item_gemini_${index}_${name}`;
          const call_id = `call_gemini_${index}_${name}`;
          const args = JSON.stringify(part.functionCall.args || {});
          events.push(event("ToolCallStarted", { item_id, call_id, name, presentation: null }));
          events.push(event("ToolCallCompleted", { item_id, call_id, name, arguments: args, presentation: null }));
          next.outputItems.push({ type: "function_call", id: item_id, call_id, name, arguments: args });
        }
      }
    }
    return { state: next, events, usage: next.usage };
  },
  finalizeStream({ state }) {
    const next = initialState(state);
    const outputItems = [...next.outputItems];
    if (next.outputText.trim()) outputItems.unshift({ type: "message", role: "assistant", content: [{ type: "output_text", text: next.outputText }] });
    return { responseId: next.responseId, outputText: next.outputText, outputItems, usage: next.usage };
  },
  buildModelsRequest({ resolved }) {
    const base = resolved.provider.apiEndpoint.replace(/\/+$/, "");
    const useKeyQuery = base.includes("generativelanguage.googleapis.com");
    const query = { ...(resolved.provider.queryParams || {}) };
    // ⚠️  Credential is NOT added here — Rust injects it server-side.
    const candidates = resolved.provider.modelsEndpointCandidates || [];
    return {
      method: "GET",
      url: withQuery(candidates[0] || `${base}/models`, query),
      headers: headerMap({}, resolved),
      authStrategy: useKeyQuery ? "key-query" : "bearer",
    };
  },
  parseModelsResponse({ response }) {
    return (response.models || response.data || []).map((model) => ({
      id: String(model.name || model.id || model).replace(/^models\//, ""),
      supportedReasoningLevels: model.supported_reasoning_levels || model.supportedReasoningLevels || [],
    })).filter((model) => model.id);
  },
  getReasoningCapability({ cachedLevels }) {
    const levels = cachedLevels || [];
    return { supportsReasoning: levels.length > 0, supportedReasoningLevels: levels };
  },
  getModelMetadata({ modelId }) {
    return resolveGeminiModelMetadata(modelId);
  },
};

globalThis.AgentJaxPlugin = {
  providers: [geminiProvider],
  tools: {},
};
