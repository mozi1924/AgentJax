/**
 * AgentJax SDK Bootstrap — evaluated into the global scope of every plugin's
 * JsRuntime before the plugin entrypoint runs.
 *
 * This makes all @agentjax/sdk functions available as direct globals so plugin
 * code can call them without needing ESM `import` statements or async module
 * loading. The source is compiled into the binary via include_str!().
 *
 * For future ESM-native plugins, these same functions are also exported from
 * `builtin-plugins/sdk/sdk.js` which is available via StaticModuleLoader.
 */

// ── URL helpers ──────────────────────────────────────────────────────────────

function withQuery(url, queryParams) {
  const pairs = Object.entries(queryParams || {})
    .filter(([key, value]) => String(key).trim() && String(value).trim())
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
  if (!pairs.length) return url;
  const separator = url.includes("?") ? (url.endsWith("?") || url.endsWith("&") ? "" : "&") : "?";
  return `${url}${separator}${pairs.join("&")}`;
}

// ── Stream events ────────────────────────────────────────────────────────────

function event(type, data) {
  return data ? { type, data } : { type };
}

// ── Text / content helpers ───────────────────────────────────────────────────

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

// ── HTTP header builder ──────────────────────────────────────────────────────

function headerMap(baseHeaders, resolved, authStrategy) {
  const strategy = authStrategy || "bearer";
  const headers = { ...baseHeaders, ...(resolved.resolvedHttpHeaders || {}) };
  const hasAuth = Object.keys(headers).some((key) =>
    ["x-api-key", "authorization"].includes(key.toLowerCase())
  );
  if (hasAuth || !resolved.credential) return headers;
  if (strategy === "x-api-key") {
    headers["x-api-key"] = resolved.credential;
  } else {
    headers.Authorization = `Bearer ${resolved.credential}`;
  }
  return headers;
}

// ── Token usage normalisation ────────────────────────────────────────────────

function usageFrom(value, previous) {
  if (!value) return null;
  var raw = (value.message && value.message.usage) || value.usage;
  if (!raw && value.response && value.response.usage) raw = value.response.usage;
  if (!raw && value.usageMetadata) {
    raw = {
      input_tokens: value.usageMetadata.promptTokenCount || 0,
      output_tokens: value.usageMetadata.candidatesTokenCount || 0,
    };
  }
  if (!raw) return null;

  var pt = raw.input_tokens || raw.prompt_tokens || raw.inputTokens || 0;
  pt = pt + (raw.cache_creation_input_tokens || 0) + (raw.cache_read_input_tokens || 0);
  if (!pt && previous && previous.promptTokens) pt = previous.promptTokens;

  var ct = raw.output_tokens || raw.completion_tokens || raw.outputTokens || 0;
  if (!ct && previous && previous.completionTokens) ct = previous.completionTokens;

  var tt = raw.total_tokens || raw.totalTokens || (pt + ct);
  if (!pt && !ct && !tt) return null;
  return { promptTokens: pt, completionTokens: ct, totalTokens: tt };
}

// ── Request config ───────────────────────────────────────────────────────────

function applyRequestConfig(payload, cfg, skip) {
  cfg = cfg || {};
  const excluded = new Set(skip || []);
  if (cfg.temperature != null && !excluded.has("temperature")) payload.temperature = cfg.temperature;
  if (cfg.topP != null && !excluded.has("topP")) payload.top_p = cfg.topP;
  if (cfg.topK != null && !excluded.has("topK")) payload.top_k = cfg.topK;
  if (cfg.maxOutputTokens != null && !excluded.has("maxOutputTokens")) payload.max_tokens = cfg.maxOutputTokens;
  if (cfg.frequencyPenalty != null && !excluded.has("frequencyPenalty")) payload.frequency_penalty = cfg.frequencyPenalty;
  if (cfg.presencePenalty != null && !excluded.has("presencePenalty")) payload.presence_penalty = cfg.presencePenalty;
  for (const [key, value] of Object.entries(cfg.extraBody || {})) {
    const normalized = key.trim().toLowerCase();
    if (normalized && !["messages", "model", "stream"].includes(normalized)) {
      payload[key] = value;
    }
  }
}
