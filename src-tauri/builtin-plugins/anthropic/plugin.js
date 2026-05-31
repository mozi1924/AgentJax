// Built-in provider plugin for Anthropic Messages-compatible APIs.
// Dependencies: AgentJax host networking primitives for HTTPS and SSE.
// Provider defaults, request conversion, model parsing, and stream parsing live here.

function withQuery(url, queryParams) {
  const pairs = Object.entries(queryParams || {})
    .filter(([key, value]) => String(key).trim() && String(value).trim())
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
  if (!pairs.length) return url;
  const separator = url.includes("?") ? (url.endsWith("?") || url.endsWith("&") ? "" : "&") : "?";
  return `${url}${separator}${pairs.join("&")}`;
}

function headerMap(baseHeaders, resolved) {
  const headers = { ...baseHeaders, ...(resolved.resolvedHttpHeaders || {}) };
  const hasCredential = Object.keys(headers).some((key) => ["x-api-key", "authorization"].includes(key.toLowerCase()));
  if (!hasCredential && resolved.credential) headers["x-api-key"] = resolved.credential;
  return headers;
}

function event(type, data) {
  return data ? { type, data } : { type };
}

function textFromContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((part) => part.text || part.input_text || "").join("");
}

function parseArgs(value) {
  if (!value) return {};
  if (typeof value === "object") return value;
  try { return JSON.parse(value); } catch { return {}; }
}

function anthropicMessages(items) {
  const messages = [];
  const systemSections = [];
  for (const item of items || []) {
    if (item.type === "function_call") {
      messages.push({ role: "assistant", content: [{ type: "tool_use", id: item.call_id, name: item.name, input: parseArgs(item.arguments) }] });
      continue;
    }
    if (item.type === "function_call_output") {
      messages.push({ role: "user", content: [{ type: "tool_result", tool_use_id: item.call_id, content: item.output || "" }] });
      continue;
    }
    const text = textFromContent(item.content);
    if (!text.trim()) continue;
    if (item.role === "system" || item.role === "developer") {
      systemSections.push(text);
    } else {
      messages.push({ role: item.role === "assistant" ? "assistant" : "user", content: [{ type: "text", text }] });
    }
  }
  return { messages, systemSections };
}

function usageFrom(value, previous) {
  const raw = value.message?.usage || value.usage;
  if (!raw) return null;
  const promptTokens = (raw.input_tokens || 0) + (raw.cache_creation_input_tokens || 0) + (raw.cache_read_input_tokens || 0) || previous?.promptTokens || 0;
  const completionTokens = raw.output_tokens || previous?.completionTokens || 0;
  const totalTokens = promptTokens + completionTokens;
  if (!promptTokens && !completionTokens) return null;
  return { promptTokens, completionTokens, totalTokens };
}

function initialState(state) {
  return {
    responseId: "",
    outputText: "",
    outputItems: [],
    usage: null,
    emittedOutputStarted: false,
    toolBlocks: {},
    ...(state || {}),
  };
}

const anthropicProvider = {
  kind: "anthropic",
  displayName: "Anthropic",
  defaultPriority: 60,
  defaultModelIds: ["claude-sonnet-4-5", "claude-opus-4-1"],
  toolSchemaFormat: "anthropic",
  configSchema: {
    type: "object",
    properties: {
      apiEndpoint: { type: "string", default: "https://api.anthropic.com/v1", title: "API Endpoint" },
      modelsEndpointCandidates: { type: "array", items: { type: "string" }, default: [] },
      queryParams: { type: "object", additionalProperties: { type: "string" }, default: {} },
      httpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      envHttpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      realtimeEndpoint: { type: ["string", "null"], default: null },
      supportsWebsockets: { type: "boolean", default: false },
      streamTransport: { type: "string", default: "sse" },
      credential: { type: ["string", "null"], default: null, sensitive: true },
      credentialEnv: { type: "string", default: "ANTHROPIC_API_KEY" },
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
    supportsJsonMode: false,
    supportsJsonSchema: false,
    supportsParallelToolCalls: true,
    supportsBuiltInWebSearch: false,
    emitsFinalOutputItems: false,
    emitsIncrementalToolCallArguments: true,
  },
  buildStreamRequest({ resolved, request }) {
    const built = anthropicMessages(request.inputItems);
    const instructions = (request.instructionsOverride || "").trim() || resolved.systemPrompt;
    if (instructions.trim()) built.systemSections.unshift(instructions);
    const cfg = resolved.requestConfig || {};
    const body = {
      model: resolved.modelId,
      max_tokens: cfg.maxOutputTokens || 4096,
      messages: built.messages,
      stream: true,
    };
    if (built.systemSections.length) body.system = built.systemSections.join("\n\n");
    if (cfg.temperature != null) body.temperature = cfg.temperature;
    if (cfg.topP != null) body.top_p = cfg.topP;
    if (cfg.topK != null) body.top_k = cfg.topK;
    if (request.tools && request.tools.length) body.tools = request.tools;
    Object.assign(body, cfg.extraBody || {});
    return {
      method: "POST",
      url: withQuery(`${resolved.provider.apiEndpoint.replace(/\/+$/, "")}/messages`, resolved.provider.queryParams),
      streamProtocol: "sse",
      headers: headerMap({ "Content-Type": "application/json", Accept: "text/event-stream", "anthropic-version": "2023-06-01" }, resolved),
      body,
    };
  },
  parseStreamEvent({ state, eventBlock }) {
    const next = initialState(state);
    const data = eventBlock.split(/\r?\n/).filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trimStart()).join("\n");
    if (!data || data === "[DONE]") return { state: next, done: data === "[DONE]" };
    const value = JSON.parse(data);
    if (value.error) throw new Error(`Anthropic streaming error: ${JSON.stringify(value.error)}`);
    const events = [];
    const type = value.type || "";
    const usage = usageFrom(value, next.usage);
    if (usage) next.usage = usage;
    if (type === "message_start") next.responseId ||= value.message?.id || "";
    if (type === "content_block_start" && value.content_block?.type === "tool_use") {
      const block = value.content_block;
      next.toolBlocks[value.index || 0] = { item_id: `item_anthropic_${value.index || 0}`, call_id: block.id || `call_anthropic_${value.index || 0}`, name: block.name || "tool", arguments: JSON.stringify(block.input || {}), completed: false };
      const tool = next.toolBlocks[value.index || 0];
      events.push(event("ToolCallStarted", { item_id: tool.item_id, call_id: tool.call_id, name: tool.name, presentation: null }));
    }
    if (type === "content_block_delta" && value.delta?.type === "text_delta" && value.delta.text) {
      if (!next.emittedOutputStarted) {
        next.emittedOutputStarted = true;
        events.push(event("OutputTextStarted"));
      }
      next.outputText += value.delta.text;
      events.push(event("OutputTextDelta", { delta: value.delta.text, phase: null }));
    }
    if (type === "content_block_delta" && value.delta?.type === "input_json_delta") {
      const tool = next.toolBlocks[value.index || 0];
      if (tool && value.delta.partial_json) {
        tool.arguments += value.delta.partial_json;
        events.push(event("ToolCallArgumentsDelta", { item_id: tool.item_id, call_id: tool.call_id, delta: value.delta.partial_json }));
      }
    }
    if (type === "content_block_stop") {
      const tool = next.toolBlocks[value.index || 0];
      if (tool && !tool.completed) {
        tool.completed = true;
        events.push(event("ToolCallCompleted", { item_id: tool.item_id, call_id: tool.call_id, name: tool.name, arguments: tool.arguments || "{}", presentation: null }));
        next.outputItems.push({ type: "function_call", id: tool.item_id, call_id: tool.call_id, name: tool.name, arguments: tool.arguments || "{}" });
      }
    }
    return { state: next, done: type === "message_stop", events, responseId: next.responseId, usage: next.usage };
  },
  finalizeStream({ state }) {
    const next = initialState(state);
    const outputItems = [...next.outputItems];
    if (next.outputText.trim()) outputItems.unshift({ type: "message", role: "assistant", content: [{ type: "output_text", text: next.outputText }] });
    return { responseId: next.responseId, outputText: next.outputText, outputItems, usage: next.usage };
  },
  buildModelsRequest({ resolved }) {
    const candidates = resolved.provider.modelsEndpointCandidates || [];
    return {
      method: "GET",
      url: withQuery(candidates[0] || `${resolved.provider.apiEndpoint.replace(/\/+$/, "")}/models`, resolved.provider.queryParams),
      headers: headerMap({ "anthropic-version": "2023-06-01" }, resolved),
    };
  },
  parseModelsResponse({ response }) {
    return (response.data || response.models || []).map((model) => ({
      id: model.id || model.model || model.name || String(model),
      supportedReasoningLevels: model.supported_reasoning_levels || model.supportedReasoningLevels || [],
    })).filter((model) => model.id);
  },
  getReasoningCapability({ cachedLevels }) {
    const levels = cachedLevels || [];
    return { supportsReasoning: levels.length > 0, supportedReasoningLevels: levels };
  },
};

globalThis.AgentJaxPlugin = {
  providers: [anthropicProvider],
  tools: {},
};
