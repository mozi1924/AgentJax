// Built-in provider plugin for OpenAI Responses-compatible APIs.
// Dependencies: AgentJax host networking primitives for HTTPS, SSE, and WebSocket.
// The plugin owns provider metadata, request payloads, model parsing, reasoning
// metadata, and stream event parsing.

function headerMap(baseHeaders, resolved) {
  const headers = { ...baseHeaders, ...(resolved.resolvedHttpHeaders || {}) };
  const hasAuth = Object.keys(headers).some((key) => key.toLowerCase() === "authorization");
  if (!hasAuth && resolved.credential) {
    headers.Authorization = `Bearer ${resolved.credential}`;
  }
  return headers;
}

function withQuery(url, queryParams) {
  const pairs = Object.entries(queryParams || {})
    .filter(([key, value]) => String(key).trim() && String(value).trim())
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
  if (!pairs.length) return url;
  const separator = url.includes("?") ? (url.endsWith("?") || url.endsWith("&") ? "" : "&") : "?";
  return `${url}${separator}${pairs.join("&")}`;
}

function event(type, data) {
  return data ? { type, data } : { type };
}

function usageFrom(value) {
  const raw = value && (value.response && value.response.usage ? value.response.usage : value.usage);
  if (!raw) return null;
  const promptTokens = raw.prompt_tokens ?? raw.input_tokens ?? raw.inputTokens ?? 0;
  const completionTokens = raw.completion_tokens ?? raw.output_tokens ?? raw.outputTokens ?? 0;
  const totalTokens = raw.total_tokens ?? raw.totalTokens ?? promptTokens + completionTokens;
  if (!promptTokens && !completionTokens && !totalTokens) return null;
  return { promptTokens, completionTokens, totalTokens };
}

function applyRequestConfig(payload, requestConfig, request) {
  const cfg = requestConfig || {};
  if (cfg.temperature != null) payload.temperature = cfg.temperature;
  if (cfg.topP != null) payload.top_p = cfg.topP;
  if (cfg.topK != null) payload.top_k = cfg.topK;
  if (cfg.maxOutputTokens != null) payload.max_output_tokens = cfg.maxOutputTokens;
  if (cfg.frequencyPenalty != null) payload.frequency_penalty = cfg.frequencyPenalty;
  if (cfg.presencePenalty != null) payload.presence_penalty = cfg.presencePenalty;
  const effort = (request.reasoningEffort || cfg.reasoningEffort || "").trim();
  if (effort) payload.reasoning = { effort };
  for (const [key, value] of Object.entries(cfg.extraBody || {})) {
    const normalized = key.trim().toLowerCase();
    if (normalized && normalized !== "store" && normalized !== "previous_response_id") {
      payload[key] = value;
    }
  }
}

function normalizeInputItems(items) {
  return (items || []).map((item) => {
    const cloned = JSON.parse(JSON.stringify(item));
    if (cloned && typeof cloned === "object") delete cloned.id;
    if (Array.isArray(cloned.content)) {
      cloned.content = cloned.content.map((part) => {
        if (part && part.type === "text") {
          return { ...part, type: cloned.role === "assistant" ? "output_text" : "input_text" };
        }
        return part;
      });
    }
    return cloned;
  });
}

function buildResponsePayload({ resolved, request }) {
  const payload = {
    model: resolved.modelId,
    instructions: (request.instructionsOverride || "").trim() || resolved.systemPrompt,
    input: normalizeInputItems(request.inputItems),
    store: false,
    stream: true,
  };
  applyRequestConfig(payload, resolved.requestConfig, request);
  if (request.tools && request.tools.length) payload.tools = request.tools;
  if (request.toolChoice != null) payload.tool_choice = request.toolChoice;
  if (request.text != null) payload.text = request.text;
  if (request.include && request.include.length) payload.include = request.include;
  if (request.serviceTier) payload.service_tier = request.serviceTier;
  if (request.promptCacheKey) payload.prompt_cache_key = request.promptCacheKey;
  if (request.clientMetadata && typeof request.clientMetadata === "object") {
    payload.client_metadata = request.clientMetadata;
  }
  if (request.generate != null) payload.generate = request.generate;
  return payload;
}

function initialState(state) {
  return {
    emittedOutputStarted: false,
    response_id: "",
    outputText: "",
    outputItems: [],
    usage: null,
    completedToolCalls: [],
    ...(state || {}),
  };
}

const openAIResponsesProvider = {
  kind: "openai-responses",
  displayName: "OpenAI Responses",
  defaultPriority: 0,
  defaultModelIds: ["gpt-5-mini", "gpt-5"],
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
  buildStreamRequest(context) {
    const base = context.resolved.provider.apiEndpoint.replace(/\/+$/, "");
    return {
      method: "POST",
      url: withQuery(`${base}/responses`, context.resolved.provider.queryParams),
      streamProtocol: "sse",
      headers: headerMap({ "Content-Type": "application/json", Accept: "text/event-stream" }, context.resolved),
      body: buildResponsePayload(context),
    };
  },
  parseStreamEvent({ state, eventBlock }) {
    const next = initialState(state);
    const data = eventBlock.split(/\r?\n/).filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart()).join("\n");
    if (!data || data === "[DONE]") return { state: next, done: data === "[DONE]" };
    const value = JSON.parse(data);
    if (value.error) throw new Error(`Streaming error: ${JSON.stringify(value.error)}`);

    const events = [];
    const type = value.type || "";
    const usage = usageFrom(value);
    if (usage) next.usage = usage;
    next.response_id ||= value.response?.id || value.response_id || value.id || "";

    if (type === "response.output_text.delta" && value.delta) {
      if (!next.emittedOutputStarted) {
        next.emittedOutputStarted = true;
        events.push(event("OutputTextStarted"));
      }
      next.outputText += value.delta;
      events.push(event("OutputTextDelta", { delta: value.delta, phase: null }));
      return { state: next, events, responseId: next.response_id, outputTextDelta: value.delta, usage: next.usage };
    }
    if (type === "response.output_item.added" && value.item?.type === "function_call") {
      events.push(event("ToolCallStarted", {
        item_id: value.item.id || "",
        call_id: value.item.call_id || "",
        name: value.item.name || "",
        presentation: null,
      }));
    }
    if (type === "response.function_call_arguments.delta" && value.delta) {
      events.push(event("ToolCallArgumentsDelta", {
        item_id: value.item_id || "",
        call_id: value.call_id || "",
        delta: value.delta,
      }));
    }
    if (type === "response.function_call_arguments.done" && value.call_id) {
      next.completedToolCalls.push(value.call_id);
      events.push(event("ToolCallCompleted", {
        item_id: value.item_id || "",
        call_id: value.call_id,
        name: "",
        arguments: value.arguments || "",
        presentation: null,
      }));
    }
    if (type === "response.output_item.done" && value.item) {
      next.outputItems.push(value.item);
      if (value.item.type === "message" && value.item.role === "assistant") {
        const text = (value.item.content || []).map((part) => part.text || "").join("");
        if (text.trim()) {
          events.push(event("AssistantMessageCompleted", { text, phase: null, response_id: next.response_id }));
        }
      }
      if (value.item.type === "function_call" && !next.completedToolCalls.includes(value.item.call_id)) {
        events.push(event("ToolCallCompleted", {
          item_id: value.item.id || "",
          call_id: value.item.call_id || "",
          name: value.item.name || "",
          arguments: value.item.arguments || "",
          presentation: null,
        }));
      }
    }
    const done = type === "response.completed" || type === "response.done";
    return { state: next, done, events, responseId: next.response_id, outputItems: [], usage: next.usage };
  },
  finalizeStream({ state }) {
    const next = initialState(state);
    return {
      responseId: next.response_id,
      outputText: next.outputText,
      outputItems: next.outputItems,
      usage: next.usage,
    };
  },
  buildModelsRequest({ resolved }) {
    const candidates = resolved.provider.modelsEndpointCandidates || [];
    const url = candidates[0] || `${resolved.provider.apiEndpoint.replace(/\/+$/, "")}/models`;
    return {
      method: "GET",
      url: withQuery(url, resolved.provider.queryParams),
      headers: headerMap({}, resolved),
    };
  },
  parseModelsResponse({ response }) {
    return (response.data || response.models || []).map((model) => ({
      id: model.id || model.name || String(model),
      supportedReasoningLevels: model.supported_reasoning_levels || model.supportedReasoningLevels || [],
    })).filter((model) => model.id);
  },
  getReasoningCapability({ modelId, cachedLevels }) {
    const levels = cachedLevels && cachedLevels.length
      ? cachedLevels
      : (/^(gpt-5|o\d|o-|codex)/i.test(modelId) ? ["minimal", "low", "medium", "high"] : []);
    return { supportsReasoning: levels.length > 0, supportedReasoningLevels: levels };
  },
};

globalThis.AgentJaxPlugin = {
  providers: [openAIResponsesProvider],
  tools: {},
};
