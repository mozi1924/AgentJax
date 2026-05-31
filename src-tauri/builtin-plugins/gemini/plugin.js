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
      return { role: "user", parts: [{ functionResponse: { name: item.name || "tool", response: { result: item.output || "" } } }] };
    }
    const role = item.role === "assistant" ? "model" : "user";
    const text = textFromContent(item.content);
    return text.trim() ? { role, parts: [{ text }] } : null;
  }).filter(Boolean);
}

function initialState(state) {
  return { responseId: "", outputText: "", outputItems: [], usage: null, emittedOutputStarted: false, nextTool: 0, ...(state || {}) };
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
    if (useKeyQuery && resolved.credential) query.key = query.key || resolved.credential;
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
      headers: headerMap({ "Content-Type": "application/json", Accept: "text/event-stream" }, resolved, useKeyQuery ? "key-query" : "bearer"),
      body,
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
    if (useKeyQuery && resolved.credential) query.key = query.key || resolved.credential;
    const candidates = resolved.provider.modelsEndpointCandidates || [];
    return {
      method: "GET",
      url: withQuery(candidates[0] || `${base}/models`, query),
      headers: headerMap({}, resolved, useKeyQuery ? "key-query" : "bearer"),
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
};

globalThis.AgentJaxPlugin = {
  providers: [geminiProvider],
  tools: {},
};
