use super::{AppConfig, SettingsOption};
use crate::models;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const OPTION_SCOPE_DELIMITER: &str = "@";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUiSnapshot {
    pub snapshot: super::SettingsSnapshot,
    pub sections: Vec<Value>,
}

pub fn build_settings_sections() -> Result<Vec<Value>, String> {
    let sections = vec![
        json!({
          "id": "general",
          "title": "General",
          "icon": "Settings2",
          "order": 10,
          "description": "全局运行行为与默认模型。",
          "children": [
            {
              "kind": "group",
              "id": "general-basics",
              "title": "Application",
              "children": [
                {
                  "kind": "field",
                  "id": "active-provider",
                  "title": "Active provider",
                  "description": "当前主对话默认使用的 provider。",
                  "path": "active_provider",
                  "valueType": "enum",
                  "control": "select",
                  "optionSourceKey": "provider_keys"
                },
                {
                  "kind": "field",
                  "id": "default-model",
                  "title": "Default model",
                  "description": "主对话默认模型。",
                  "path": "default_model",
                  "valueType": "enum",
                  "control": "select",
                  "optionSourceKey": "model_refs"
                },
                {
                  "kind": "field",
                  "id": "utility-small-model",
                  "title": "Utility small model",
                  "description": "标题生成和轻量任务优先使用的小模型。",
                  "path": "utility_small_model",
                  "valueType": "enum",
                  "control": "select",
                  "optionSourceKey": "model_refs"
                },
                {
                  "kind": "field",
                  "id": "request-timeout-seconds",
                  "title": "Request timeout (seconds)",
                  "description": "所有 provider 的全局回退超时。",
                  "path": "request_timeout_seconds",
                  "valueType": "integer",
                  "control": "number",
                  "min": 1,
                  "max": 3600,
                  "step": 1
                },
                {
                  "kind": "field",
                  "id": "system-prompt",
                  "title": "System prompt",
                  "description": "应用的全局默认系统提示词。",
                  "path": "system_prompt",
                  "valueType": "string",
                  "control": "textarea",
                  "rows": 5,
                  "placeholder": "Enter default system prompt…"
                }
              ]
            }
          ]
        }),
        json!({
          "id": "providers",
          "title": "Providers",
          "icon": "PlugZap",
          "order": 20,
          "description": "连接 provider、认证方式和模型目录入口。",
          "children": [
            {
              "kind": "collection",
              "id": "providers-collection",
              "title": "Configured providers",
              "description": "每个 provider 都可以定义自己的一组模型与传输策略。",
              "path": "providers",
              "valueType": "object_collection",
              "addLabel": "Add provider",
              "keyLabel": "Provider key",
              "itemLabel": "Provider",
              "keyPattern": "^[A-Za-z0-9_-]+$",
              "defaultItem": {
                "kind": "openai",
                "api_endpoint": "https://api.openai.com/v1",
                "models_endpoint_candidates": [],
                "realtime_endpoint": null,
                "stream_transport": "websocket",
                "credential": null,
                "credential_env": "OPENAI_API_KEY",
                "request_timeout_seconds": null,
                "models": {}
              },
              "children": [
                {
                  "kind": "field",
                  "id": "provider-kind",
                  "title": "Kind",
                  "path": "kind",
                  "valueType": "enum",
                  "control": "select",
                  "optionSourceKey": "provider_kind"
                },
                {
                  "kind": "field",
                  "id": "provider-api-endpoint",
                  "title": "API endpoint",
                  "path": "api_endpoint",
                  "valueType": "string",
                  "control": "text",
                  "placeholder": "https://api.example.com/v1"
                },
                {
                  "kind": "field",
                  "id": "provider-models-endpoints",
                  "title": "Models endpoint candidates",
                  "description": "可选的备用模型列表接口。",
                  "path": "models_endpoint_candidates",
                  "valueType": "string_list",
                  "control": "tags"
                },
                {
                  "kind": "field",
                  "id": "provider-realtime-endpoint",
                  "title": "Realtime endpoint",
                  "description": "留空时会根据 API endpoint 自动推导。",
                  "path": "realtime_endpoint",
                  "valueType": "string",
                  "control": "text",
                  "placeholder": "wss://api.example.com/v1/realtime"
                },
                {
                  "kind": "field",
                  "id": "provider-stream-transport",
                  "title": "Stream transport",
                  "path": "stream_transport",
                  "valueType": "enum",
                  "control": "select",
                  "optionSourceKey": "stream_transport"
                },
                {
                  "kind": "field",
                  "id": "provider-credential",
                  "title": "Inline credential",
                  "description": "默认不回显当前值，只在你输入新值时覆盖。",
                  "path": "credential",
                  "valueType": "secret",
                  "control": "secret",
                  "placeholder": "Paste a new API key to replace the current secret"
                },
                {
                  "kind": "field",
                  "id": "provider-credential-env",
                  "title": "Credential env",
                  "description": "当 inline credential 为空时，从这个环境变量读取。",
                  "path": "credential_env",
                  "valueType": "string",
                  "control": "text"
                },
                {
                  "kind": "field",
                  "id": "provider-timeout",
                  "title": "Provider timeout override",
                  "description": "留空则跟随全局超时。",
                  "path": "request_timeout_seconds",
                  "valueType": "integer",
                  "control": "number",
                  "min": 1,
                  "max": 3600,
                  "step": 1
                }
              ]
            }
          ]
        }),
        json!({
          "id": "model-profiles",
          "title": "Model Profiles",
          "icon": "Bot",
          "order": 30,
          "description": "按 provider 管理模型预设与请求参数。",
          "children": [
            {
              "kind": "collection",
              "id": "provider-profile-models",
              "title": "Provider models",
              "path": "providers",
              "valueType": "object_collection",
              "addLabel": "Add provider shell",
              "keyLabel": "Provider key",
              "itemLabel": "Provider",
              "keyPattern": "^[A-Za-z0-9_-]+$",
              "defaultItem": {
                "kind": "openai",
                "api_endpoint": "https://api.openai.com/v1",
                "models_endpoint_candidates": [],
                "realtime_endpoint": null,
                "stream_transport": "websocket",
                "credential": null,
                "credential_env": "OPENAI_API_KEY",
                "request_timeout_seconds": null,
                "models": {}
              },
              "children": [
                {
                  "kind": "collection",
                  "id": "provider-models",
                  "title": "Profiles",
                  "path": "models",
                  "valueType": "object_collection",
                  "addLabel": "Add model profile",
                  "keyLabel": "Profile key",
                  "itemLabel": "Model profile",
                  "keyPattern": "^[A-Za-z0-9_-]+$",
                  "defaultItem": {
                    "model": "",
                    "enabled": true,
                    "request": {
                      "temperature": null,
                      "top_p": null,
                      "top_k": null,
                      "max_output_tokens": null,
                      "frequency_penalty": null,
                      "presence_penalty": null,
                      "reasoning_effort": null,
                      "extra_body": {}
                    }
                  },
                  "children": [
                    {
                      "kind": "field",
                      "id": "profile-model-id",
                      "title": "Model id",
                      "description": "实际发送给 provider 的模型名称。",
                      "path": "model",
                      "valueType": "string",
                      "control": "text",
                      "placeholder": "gpt-5-mini"
                    },
                    {
                      "kind": "field",
                      "id": "profile-enabled",
                      "title": "Enabled",
                      "path": "enabled",
                      "valueType": "boolean",
                      "control": "switch"
                    },
                    {
                      "kind": "group",
                      "id": "profile-request-group",
                      "title": "Request overrides",
                      "children": [
                        {
                          "kind": "field",
                          "id": "profile-temperature",
                          "title": "Temperature",
                          "path": "request.temperature",
                          "valueType": "float",
                          "control": "number",
                          "min": 0,
                          "max": 2,
                          "step": 0.1
                        },
                        {
                          "kind": "field",
                          "id": "profile-top-p",
                          "title": "Top P",
                          "path": "request.top_p",
                          "valueType": "float",
                          "control": "number",
                          "min": 0,
                          "max": 1,
                          "step": 0.05
                        },
                        {
                          "kind": "field",
                          "id": "profile-top-k",
                          "title": "Top K",
                          "path": "request.top_k",
                          "valueType": "integer",
                          "control": "number",
                          "min": 1,
                          "max": 999999,
                          "step": 1
                        },
                        {
                          "kind": "field",
                          "id": "profile-max-output-tokens",
                          "title": "Max output tokens",
                          "path": "request.max_output_tokens",
                          "valueType": "integer",
                          "control": "number",
                          "min": 1,
                          "max": 999999,
                          "step": 1
                        },
                        {
                          "kind": "field",
                          "id": "profile-frequency-penalty",
                          "title": "Frequency penalty",
                          "path": "request.frequency_penalty",
                          "valueType": "float",
                          "control": "number",
                          "min": -2,
                          "max": 2,
                          "step": 0.1
                        },
                        {
                          "kind": "field",
                          "id": "profile-presence-penalty",
                          "title": "Presence penalty",
                          "path": "request.presence_penalty",
                          "valueType": "float",
                          "control": "number",
                          "min": -2,
                          "max": 2,
                          "step": 0.1
                        },
                        {
                          "kind": "field",
                          "id": "profile-reasoning-effort",
                          "title": "Reasoning effort",
                          "description": "留空则交给运行时默认逻辑。",
                          "path": "request.reasoning_effort",
                          "valueType": "enum",
                          "control": "select",
                          "optionSourceKey": "reasoning_effort"
                        },
                        {
                          "kind": "field",
                          "id": "profile-extra-body",
                          "title": "Extra body",
                          "description": "Provider 特有字段透传。",
                          "path": "request.extra_body",
                          "valueType": "json_map",
                          "control": "json"
                        }
                      ]
                    }
                  ]
                }
              ]
            }
          ]
        }),
        json!({
          "id": "mcp-runtime",
          "title": "MCP Runtime",
          "icon": "Cpu",
          "order": 40,
          "description": "所有本地 stdio MCP 进程共享的运行时默认值。",
          "children": [
            {
              "kind": "group",
              "id": "mcp-runtime-group",
              "title": "Shared runtime",
              "children": [
                {
                  "kind": "field",
                  "id": "mcp-runtime-inherit-parent-env",
                  "title": "Inherit parent env",
                  "path": "mcp_runtime.stdio.inherit_parent_env",
                  "valueType": "boolean",
                  "control": "switch"
                },
                {
                  "kind": "field",
                  "id": "mcp-runtime-env",
                  "title": "Shared env",
                  "path": "mcp_runtime.stdio.env",
                  "valueType": "string_map",
                  "control": "key_value"
                },
                {
                  "kind": "field",
                  "id": "mcp-runtime-startup-timeout",
                  "title": "Startup timeout (ms)",
                  "path": "mcp_runtime.startup_timeout_ms",
                  "valueType": "integer",
                  "control": "number",
                  "min": 100,
                  "max": 600000,
                  "step": 100
                },
                {
                  "kind": "field",
                  "id": "mcp-runtime-tool-timeout",
                  "title": "Tool timeout (ms)",
                  "path": "mcp_runtime.tool_timeout_ms",
                  "valueType": "integer",
                  "control": "number",
                  "min": 100,
                  "max": 600000,
                  "step": 100
                }
              ]
            }
          ]
        }),
        json!({
          "id": "mcp-servers",
          "title": "MCP Servers",
          "icon": "ServerCog",
          "order": 50,
          "description": "配置本地 stdio 或远程 streamable HTTP MCP 服务。",
          "children": [
            {
              "kind": "collection",
              "id": "mcp-servers-collection",
              "title": "Servers",
              "path": "mcp_servers",
              "valueType": "object_collection",
              "addLabel": "Add MCP server",
              "keyLabel": "Server key",
              "itemLabel": "Server",
              "keyPattern": "^[A-Za-z0-9_-]+$",
              "defaultItem": {
                "transport": "stdio",
                "command": "",
                "args": [],
                "env": {},
                "cwd": null,
                "use_global_stdio_env": true,
                "inherit_parent_env": null,
                "uri": null,
                "auth_header": null,
                "headers": {},
                "allow_stateless": true,
                "channel_buffer_capacity": null,
                "reinit_on_expired_session": true,
                "enabled": true
              },
              "children": [
                {
                  "kind": "field",
                  "id": "mcp-server-enabled",
                  "title": "Enabled",
                  "path": "enabled",
                  "valueType": "boolean",
                  "control": "switch"
                },
                {
                  "kind": "field",
                  "id": "mcp-server-transport",
                  "title": "Transport",
                  "path": "transport",
                  "valueType": "enum",
                  "control": "select",
                  "optionSourceKey": "mcp_transport"
                },
                {
                  "kind": "field",
                  "id": "mcp-server-command",
                  "title": "Command",
                  "path": "command",
                  "valueType": "string",
                  "control": "text",
                  "visibleWhen": [{ "path": "transport", "equals": "stdio" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-args",
                  "title": "Args",
                  "path": "args",
                  "valueType": "string_list",
                  "control": "tags",
                  "visibleWhen": [{ "path": "transport", "equals": "stdio" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-env",
                  "title": "Env",
                  "path": "env",
                  "valueType": "string_map",
                  "control": "key_value"
                },
                {
                  "kind": "field",
                  "id": "mcp-server-cwd",
                  "title": "Working directory",
                  "path": "cwd",
                  "valueType": "string",
                  "control": "text",
                  "visibleWhen": [{ "path": "transport", "equals": "stdio" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-use-global-env",
                  "title": "Use global stdio env",
                  "path": "use_global_stdio_env",
                  "valueType": "boolean",
                  "control": "switch",
                  "visibleWhen": [{ "path": "transport", "equals": "stdio" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-inherit-parent-env",
                  "title": "Inherit parent env",
                  "description": "留空时跟随共享 runtime 配置。",
                  "path": "inherit_parent_env",
                  "valueType": "enum",
                  "control": "select",
                  "options": [
                    { "label": "Follow runtime", "value": "" },
                    { "label": "true", "value": "true" },
                    { "label": "false", "value": "false" }
                  ],
                  "visibleWhen": [{ "path": "transport", "equals": "stdio" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-uri",
                  "title": "URI",
                  "path": "uri",
                  "valueType": "string",
                  "control": "text",
                  "visibleWhen": [{ "path": "transport", "equals": "streamable_http" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-auth-header",
                  "title": "Auth header",
                  "description": "如需 Bearer token，可直接填写完整 header 值。",
                  "path": "auth_header",
                  "valueType": "secret",
                  "control": "secret",
                  "visibleWhen": [{ "path": "transport", "equals": "streamable_http" }],
                  "placeholder": "Bearer ..."
                },
                {
                  "kind": "field",
                  "id": "mcp-server-headers",
                  "title": "HTTP headers",
                  "path": "headers",
                  "valueType": "string_map",
                  "control": "key_value",
                  "visibleWhen": [{ "path": "transport", "equals": "streamable_http" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-allow-stateless",
                  "title": "Allow stateless",
                  "path": "allow_stateless",
                  "valueType": "boolean",
                  "control": "switch",
                  "visibleWhen": [{ "path": "transport", "equals": "streamable_http" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-channel-buffer",
                  "title": "Channel buffer capacity",
                  "description": "留空表示使用默认值。",
                  "path": "channel_buffer_capacity",
                  "valueType": "integer",
                  "control": "number",
                  "min": 1,
                  "max": 100000,
                  "step": 1,
                  "visibleWhen": [{ "path": "transport", "equals": "streamable_http" }]
                },
                {
                  "kind": "field",
                  "id": "mcp-server-reinit",
                  "title": "Reinit on expired session",
                  "path": "reinit_on_expired_session",
                  "valueType": "boolean",
                  "control": "switch",
                  "visibleWhen": [{ "path": "transport", "equals": "streamable_http" }]
                }
              ]
            }
          ]
        }),
    ];

    validate_unique_schema_ids(&sections)?;
    Ok(sections)
}

fn scoped_option_source(base_key: &str, context_path: &str) -> String {
    format!("{base_key}{OPTION_SCOPE_DELIMITER}{context_path}")
}

pub fn build_dynamic_options(
    config: &AppConfig,
) -> Result<BTreeMap<String, Vec<SettingsOption>>, String> {
    let mut dynamic_options = BTreeMap::new();

    let provider_options = config
        .provider_keys()
        .into_iter()
        .map(|provider_key| SettingsOption {
            label: provider_key.clone(),
            value: provider_key,
        })
        .collect::<Vec<_>>();
    dynamic_options.insert("provider_keys".to_string(), provider_options);

    let model_options = config
        .configured_models()
        .into_iter()
        .map(|model_ref| SettingsOption {
            label: model_ref.clone(),
            value: model_ref,
        })
        .collect::<Vec<_>>();
    dynamic_options.insert("model_refs".to_string(), model_options);

    dynamic_options.insert(
        "provider_kind".to_string(),
        ["openai", "codex"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect(),
    );
    dynamic_options.insert(
        "stream_transport".to_string(),
        ["websocket", "sse"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect(),
    );
    dynamic_options.insert(
        "mcp_transport".to_string(),
        ["stdio", "streamable_http"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect(),
    );

    let reasoning_entries = models::get_model_catalog_entries_from_config(config)?;
    let mut global_reasoning_levels = Vec::new();

    for entry in reasoning_entries {
        let context_path = format!(
            "providers.{}.models.{}",
            entry.provider_key,
            profile_key_from_ref(&entry.profile_key)
        );
        let options = reasoning_options_with_default(&entry.supported_reasoning_levels);

        dynamic_options.insert(
            scoped_option_source("reasoning_effort", &context_path),
            options.clone(),
        );

        if options.len() > 1 {
            for option in options {
                if option.value.is_empty()
                    || global_reasoning_levels
                        .iter()
                        .any(|existing: &SettingsOption| existing.value == option.value)
                {
                    continue;
                }
                global_reasoning_levels.push(option);
            }
        }
    }

    let mut global_options = vec![SettingsOption {
        label: "Follow default".to_string(),
        value: "".to_string(),
    }];
    global_options.extend(global_reasoning_levels);
    dynamic_options.insert("reasoning_effort".to_string(), global_options);

    Ok(dynamic_options)
}

fn reasoning_options_with_default(levels: &[String]) -> Vec<SettingsOption> {
    let mut options = vec![SettingsOption {
        label: "Follow default".to_string(),
        value: "".to_string(),
    }];

    for level in levels {
        let normalized = level.trim().to_lowercase();
        if normalized.is_empty() || options.iter().any(|existing| existing.value == normalized) {
            continue;
        }

        options.push(SettingsOption {
            label: normalized.clone(),
            value: normalized,
        });
    }

    options
}

fn profile_key_from_ref(profile_ref: &str) -> String {
    profile_ref
        .split_once('/')
        .map(|(_, profile_key)| profile_key.to_string())
        .unwrap_or_else(|| profile_ref.to_string())
}

fn validate_unique_schema_ids(sections: &[Value]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    for section in sections {
        walk_node_ids(section, &mut ids)?;
    }
    Ok(())
}

fn walk_node_ids(node: &Value, ids: &mut std::collections::BTreeSet<String>) -> Result<(), String> {
    let object = node
        .as_object()
        .ok_or_else(|| "Settings schema node must be an object".to_string())?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Settings schema node is missing non-empty id".to_string())?;

    if !ids.insert(id.to_string()) {
        return Err(format!("Duplicate settings schema node id: {id}"));
    }

    if let Some(children) = object.get("children") {
        let array = children
            .as_array()
            .ok_or_else(|| format!("Settings schema node '{id}' children must be an array"))?;
        for child in array {
            walk_node_ids(child, ids)?;
        }
    }

    Ok(())
}
