/**
 * @agentjax/sdk — Shared utilities for AgentJax provider plugins.
 *
 * This module is pre-registered via StaticModuleLoader so all built-in
 * and user plugins can import shared helpers without bundling duplicates.
 *
 * All functions are pure — no mutable state, no side effects.
 */

/**
 * Append query parameters to a URL, preserving any existing query string.
 * @param {string} url
 * @param {Record<string,string>} [queryParams]
 * @returns {string}
 */
export function withQuery(url, queryParams) {
  const pairs = Object.entries(queryParams || {})
    .filter(([key, value]) => String(key).trim() && String(value).trim())
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
  if (!pairs.length) return url;
  const separator = url.includes("?") ? (url.endsWith("?") || url.endsWith("&") ? "" : "&") : "?";
  return `${url}${separator}${pairs.join("&")}`;
}

/**
 * Build a normalized stream event object.
 * @param {string} type
 * @param {object} [data]
 * @returns {{ type: string, data?: object }}
 */
export function event(type, data) {
  return data ? { type, data } : { type };
}

/**
 * Extract text from a content block (handles string or array-of-parts).
 * @param {string|Array} content
 * @returns {string}
 */
export function textFromContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((part) => part.text || part.input_text || "").join("");
}

/**
 * Parse a value that may be a JSON string or already an object.
 * @param {any} value
 * @returns {object}
 */
export function parseArgs(value) {
  if (!value) return {};
  if (typeof value === "object") return value;
  try { return JSON.parse(value); } catch { return {}; }
}

/**
 * Build HTTP headers for a provider request, applying resolved credentials.
 *
 * Strategy selection:
 *   - "x-api-key": writes resolved.credential into x-api-key header
 *   - "bearer":    writes "Bearer {credential}" into Authorization
 *   - "key-query": appends ?key= to the URL (Gemini API key style)
 *
 * @param {Record<string,string>} baseHeaders
 * @param {object} resolved       – resolved model config from the host
 * @param {("x-api-key"|"bearer"|"key-query")} [authStrategy="bearer"]
 * @returns {Record<string,string>}
 */
export function headerMap(baseHeaders, resolved, authStrategy) {
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

/**
 * Extract normalized usage from a provider response chunk.
 *
 * Supports these upstream shapes:
 *   - Anthropic:    { message: { usage: { input_tokens, output_tokens } } }
 *   - Chat Completions: { usage: { prompt_tokens, completion_tokens } }
 *   - Gemini:      { usageMetadata: { promptTokenCount, candidatesTokenCount } }
 *   - Responses:   { response: { usage: { input_tokens, output_tokens } } }
 *
 * @param {object} value         – raw upstream chunk
 * @param {object} [previous]    – previous usage (for incremental updates)
 * @returns {{ promptTokens: number, completionTokens: number, totalTokens: number } | null}
 */
export function usageFrom(value, previous) {
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

/**
 * Apply a shared subset of ModelRequestConfig to a mutable payload object.
 *
 * @param {object} payload     – mutated in place
 * @param {object|null} cfg    – resolved.requestConfig
 * @param {string[]} [skip]    – keys to skip (e.g. provider-specific fields)
 */
export function applyRequestConfig(payload, cfg, skip) {
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
