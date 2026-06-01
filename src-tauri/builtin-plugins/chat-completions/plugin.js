// Built-in provider plugin for Chat Completions-compatible APIs.
// Dependencies: agentjax SDK bootstrap provides withQuery, event, textFromContent,
// headerMap, usageFrom, applyRequestConfig as globals.
// The plugin owns provider-specific request conversion, stream parsing, etc.

function responseItemsToMessages(items) {
  const messages = [];
  for (const item of items || []) {
    if (item.type === "function_call") {
      messages.push({
        role: "assistant",
        tool_calls: [{
          id: item.call_id,
          type: "function",
          function: { name: item.name, arguments: typeof item.arguments === "string" ? item.arguments : JSON.stringify(item.arguments || {}) },
        }],
      });
      continue;
    }
    if (item.type === "function_call_output") {
      messages.push({ role: "tool", tool_call_id: item.call_id, content: item.output || "" });
      continue;
    }
    const role = item.role === "assistant" ? "assistant" : item.role === "system" ? "system" : "user";
    const content = textFromContent(item.content);
    if (content.trim()) messages.push({ role, content });
  }
  return messages;
}

function initialState(state) {
  return {
    responseId: "",
    outputText: "",
    emittedOutputStarted: false,
    usage: null,
    toolCalls: {},
    outputItems: [],
    ...(state || {}),
  };
}

// ── Chat Completions Model Registry ─────────────────────────────────
const CHAT_COMPLETIONS_MODELS = {
  // OpenAI series
  "gpt-5": { contextWindow: 400000 },
  "gpt-5.5": { contextWindow: 1000000 },
  "gpt-5-mini": { contextWindow: 400000 },
  "o3-mini": { contextWindow: 200000 },
  "o1": { contextWindow: 128000 },
  "o1-mini": { contextWindow: 128000 },
  "gpt-4o": { contextWindow: 128000 },
  "gpt-4o-mini": { contextWindow: 128000 },
  "gpt-4-turbo": { contextWindow: 128000 },
  "gpt-4": { contextWindow: 8192 },
  "gpt-3.5-turbo": { contextWindow: 16384 },

  // DeepSeek series
  "deepseek-chat": { contextWindow: 128000 },
  "deepseek-reasoner": { contextWindow: 128000 },
  "deepseek-r1": { contextWindow: 128000 },
  "deepseek-v3": { contextWindow: 128000 },

  // Mistral series
  "mistral-large": { contextWindow: 128000 },
  "codestral": { contextWindow: 32000 },

  // Llama series
  "llama-4": { contextWindow: 1000000 },
  "llama-4-scout": { contextWindow: 10000000 }, // 10M Scout
  "llama-3.3": { contextWindow: 128000 },
  "llama-3.2": { contextWindow: 128000 },
  "llama-3.1": { contextWindow: 128000 },
  "llama-3": { contextWindow: 8192 },

  // Qwen series
  "qwen-3": { contextWindow: 1000000 }, // 1M Qwen 3
  "qwen-2.5": { contextWindow: 128000 },
};

function resolveChatCompletionsModelMetadata(modelId) {
  const normalized = (modelId || "").trim().toLowerCase();
  if (CHAT_COMPLETIONS_MODELS[normalized]) return CHAT_COMPLETIONS_MODELS[normalized];

  // OpenAI models
  if (normalized.includes("gpt") || normalized.startsWith("o1") || normalized.startsWith("o3")) {
    if (normalized.includes("gpt-5.5")) return CHAT_COMPLETIONS_MODELS["gpt-5.5"];
    if (normalized.includes("gpt-5-mini")) return CHAT_COMPLETIONS_MODELS["gpt-5-mini"];
    if (normalized.includes("gpt-5")) return CHAT_COMPLETIONS_MODELS["gpt-5"];
    if (normalized.startsWith("o3-mini")) return CHAT_COMPLETIONS_MODELS["o3-mini"];
    if (normalized.startsWith("o1")) return CHAT_COMPLETIONS_MODELS["o1"];
    if (normalized.includes("gpt-4o-mini")) return CHAT_COMPLETIONS_MODELS["gpt-4o-mini"];
    if (normalized.includes("gpt-4o")) return CHAT_COMPLETIONS_MODELS["gpt-4o"];
    if (normalized.includes("gpt-4-32k")) return { contextWindow: 32768 };
    if (normalized.includes("gpt-4-turbo") || normalized.includes("gpt-4-1106")) return CHAT_COMPLETIONS_MODELS["gpt-4-turbo"];
    if (normalized.includes("gpt-4")) return CHAT_COMPLETIONS_MODELS["gpt-4"];
    if (normalized.includes("gpt-3.5")) return CHAT_COMPLETIONS_MODELS["gpt-3.5-turbo"];
  }

  // DeepSeek
  if (normalized.includes("deepseek")) {
    return CHAT_COMPLETIONS_MODELS["deepseek-chat"];
  }

  // Llama
  if (normalized.includes("llama")) {
    if (normalized.includes("llama-4") || normalized.includes("llama-v4")) {
      if (normalized.includes("scout")) {
        return CHAT_COMPLETIONS_MODELS["llama-4-scout"];
      }
      return CHAT_COMPLETIONS_MODELS["llama-4"];
    }
    if (normalized.includes("3.1") || normalized.includes("3.2") || normalized.includes("3.3")) {
      return CHAT_COMPLETIONS_MODELS["llama-3.3"];
    }
    return CHAT_COMPLETIONS_MODELS["llama-3"];
  }

  // Qwen
  if (normalized.includes("qwen")) {
    if (normalized.includes("qwen-3") || normalized.includes("qwen3")) {
      return CHAT_COMPLETIONS_MODELS["qwen-3"];
    }
    return CHAT_COMPLETIONS_MODELS["qwen-2.5"];
  }

  // Mistral
  if (normalized.includes("mistral") || normalized.includes("codestral") || normalized.includes("mixtral")) {
    if (normalized.includes("large")) {
      return CHAT_COMPLETIONS_MODELS["mistral-large"];
    }
    return CHAT_COMPLETIONS_MODELS["codestral"];
  }

  if (normalized.includes("nova") || normalized.includes("amazon") || normalized.includes("command") || normalized.includes("cohere") || normalized.includes("grok") || normalized.includes("jamba") || normalized.includes("ai21")) {
    return { contextWindow: 128000 };
  }

  return { contextWindow: 128000 };
}

const chatCompletionsProvider = {
  kind: "chat-completions",
  displayName: "Chat Completions",
  defaultPriority: 20,
  defaultModelIds: ["gpt-4.1", "gpt-4o"],
  toolSchemaFormat: "chat_completions",
  configSchema: {
    type: "object",
    properties: {
      apiEndpoint: { type: "string", default: "https://api.openai.com/v1", title: "API Endpoint" },
      modelsEndpointCandidates: { type: "array", items: { type: "string" }, default: [] },
      queryParams: { type: "object", additionalProperties: { type: "string" }, default: {} },
      httpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      envHttpHeaders: { type: "object", additionalProperties: { type: "string" }, default: {} },
      realtimeEndpoint: { type: ["string", "null"], default: null },
      supportsWebsockets: { type: "boolean", default: false },
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
    emitsIncrementalToolCallArguments: true,
  },
  buildStreamRequest(context) {
    const messages = [];
    const instructions = (context.request.instructionsOverride || "").trim() || context.resolved.systemPrompt;
    if (instructions.trim()) messages.push({ role: "system", content: instructions });
    messages.push(...responseItemsToMessages(context.request.inputItems));
    const body = { model: context.resolved.modelId, messages, stream: true, stream_options: { include_usage: true } };
    applyRequestConfig(body, context.resolved.requestConfig);
    if (context.request.tools && context.request.tools.length) body.tools = context.request.tools;
    if (context.request.toolChoice != null) body.tool_choice = context.request.toolChoice;
    if (context.request.text?.format?.type === "json_object") body.response_format = { type: "json_object" };
    return {
      method: "POST",
      url: withQuery(`${context.resolved.provider.apiEndpoint.replace(/\/+$/, "")}/chat/completions`, context.resolved.provider.queryParams),
      streamProtocol: "sse",
      headers: headerMap({ "Content-Type": "application/json", Accept: "text/event-stream" }, context.resolved),
      body,
    };
  },
  parseStreamEvent({ state, eventBlock }) {
    const next = initialState(state);
    const data = eventBlock.split(/\r?\n/).filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart()).join("\n");
    if (!data || data === "[DONE]") return { state: next, done: data === "[DONE]" };
    const value = JSON.parse(data);
    if (value.error) throw new Error(`Chat Completions streaming error: ${JSON.stringify(value.error)}`);
    next.responseId ||= value.id || "";
    const usage = usageFrom(value);
    if (usage) next.usage = usage;
    const events = [];
    let done = false;
    for (const choice of value.choices || []) {
      const delta = choice.delta || {};
      if (delta.content) {
        if (!next.emittedOutputStarted) {
          next.emittedOutputStarted = true;
          events.push(event("OutputTextStarted"));
        }
        next.outputText += delta.content;
        events.push(event("OutputTextDelta", { delta: delta.content, phase: null }));
      }
      for (const toolCall of delta.tool_calls || []) {
        const index = toolCall.index || 0;
        const entry = next.toolCalls[index] || { item_id: `item_chat_${index}`, call_id: toolCall.id || `call_chat_${index}`, name: "", arguments: "", started: false };
        if (toolCall.id) entry.call_id = toolCall.id;
        if (toolCall.function?.name) entry.name += toolCall.function.name;
        if (!entry.started && entry.name) {
          entry.started = true;
          events.push(event("ToolCallStarted", { item_id: entry.item_id, call_id: entry.call_id, name: entry.name, presentation: null }));
        }
        if (toolCall.function?.arguments) {
          entry.arguments += toolCall.function.arguments;
          events.push(event("ToolCallArgumentsDelta", { item_id: entry.item_id, call_id: entry.call_id, delta: toolCall.function.arguments }));
        }
        next.toolCalls[index] = entry;
      }
      if (choice.finish_reason === "tool_calls") {
        for (const entry of Object.values(next.toolCalls)) {
          if (!entry.completed && entry.name) {
            entry.completed = true;
            events.push(event("ToolCallCompleted", { item_id: entry.item_id, call_id: entry.call_id, name: entry.name, arguments: entry.arguments, presentation: null }));
            next.outputItems.push({ type: "function_call", id: entry.item_id, call_id: entry.call_id, name: entry.name, arguments: entry.arguments });
          }
        }
      }
      if (choice.finish_reason) done = true;
    }
    return { state: next, done, events, responseId: next.responseId, outputTextDelta: "", usage: next.usage };
  },
  finalizeStream({ state }) {
    const next = initialState(state);
    const outputItems = [...next.outputItems];
    if (next.outputText.trim()) {
      outputItems.unshift({ type: "message", role: "assistant", content: [{ type: "output_text", text: next.outputText }] });
    }
    return { responseId: next.responseId, outputText: next.outputText, outputItems, usage: next.usage };
  },
  buildModelsRequest({ resolved }) {
    const candidates = resolved.provider.modelsEndpointCandidates || [];
    const url = candidates[0] || `${resolved.provider.apiEndpoint.replace(/\/+$/, "")}/models`;
    return { method: "GET", url: withQuery(url, resolved.provider.queryParams), headers: headerMap({}, resolved) };
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
  getModelMetadata({ modelId }) {
    return resolveChatCompletionsModelMetadata(modelId);
  },
};

globalThis.AgentJaxPlugin = {
  providers: [chatCompletionsProvider],
  tools: {},
};
