use super::*;
use crate::agentjax_home::AGENTJAX_HOME_ENV;
use crate::conversation_store_utils::now_unix_ms;
use crate::message_phase::AssistantPhase;
use serde_json::json;
use uuid::Uuid;

const TEST_MAX_CONTEXT_ITEMS_PER_REQUEST: usize = 200;

struct TestHomeGuard {
    home: std::path::PathBuf,
}
impl Drop for TestHomeGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
fn setup_test_home() -> TestHomeGuard {
    let home = std::env::temp_dir().join(format!("agentjax-cs-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("create test home");
    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }
    TestHomeGuard { home }
}

fn u(id: &str, req: &str, text: &str) -> ConversationLine {
    ConversationLine::User(UserLine {
        id: id.into(),
        ts: now_unix_ms(),
        request_id: req.into(),
        text: text.into(),
    })
}
fn a(
    id: &str,
    req: &str,
    resp: &str,
    phase: Option<AssistantPhase>,
    text: &str,
    st: AssistantStatus,
) -> ConversationLine {
    ConversationLine::Assistant(AssistantLine {
        id: id.into(),
        ts: now_unix_ms(),
        request_id: req.into(),
        response_id: resp.into(),
        phase,
        text: text.into(),
        status: st,
    })
}
fn t(
    id: &str,
    req: &str,
    call: &str,
    name: &str,
    args: serde_json::Value,
    out: Option<serde_json::Value>,
    st: ToolStatus,
) -> ConversationLine {
    let ts = now_unix_ms();
    ConversationLine::Tool(ToolLine {
        id: id.into(),
        ts,
        started_ts: ts,
        completed_ts: matches!(st, ToolStatus::Done | ToolStatus::Failed).then_some(ts),
        request_id: req.into(),
        call_id: call.into(),
        name: name.into(),
        display_name: None,
        description: None,
        icon: None,
        args,
        output: out,
        status: st,
    })
}

#[test]
fn delete_conversation_removes_session_directory() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("td-{}", Uuid::new_v4());
    let p = conversation_dir_path(&cid).expect("path");
    ensure_conversation(&cid).expect("ensure");
    assert!(p.exists());
    assert!(delete_conversation(&cid).expect("del"));
    assert!(!p.exists());
}

#[test]
fn load_context_merges_user_and_assistant() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tc-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "r1", "hi"),
    })
    .expect("u");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::FinalAnswer),
            "hey",
            AssistantStatus::Done,
        ),
    })
    .expect("a");
    let ctx = load_context_for_request(&cid).expect("ctx");
    assert!(ctx.input_items.len() >= 2);
    assert!(ctx
        .input_items
        .iter()
        .any(|i| i.get("role").and_then(|v| v.as_str()) == Some("user")));
    assert!(ctx.input_items.iter().any(|i| {
        i.get("role").and_then(|v| v.as_str()) == Some("assistant")
            && i.get("phase").and_then(|v| v.as_str()) == Some("final_answer")
    }));
    delete_conversation(&cid).ok();
}

#[test]
fn load_context_replays_commentary_and_final_with_phase() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tphase-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::Commentary),
            "checking files",
            AssistantStatus::Done,
        ),
    })
    .expect("commentary");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a2",
            "r1",
            "resp1",
            Some(AssistantPhase::FinalAnswer),
            "done",
            AssistantStatus::Done,
        ),
    })
    .expect("final");

    let ctx = load_context_for_request(&cid).expect("ctx");
    let assistant_phases: Vec<&str> = ctx
        .input_items
        .iter()
        .filter(|item| item.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .filter_map(|item| item.get("phase").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(assistant_phases, vec!["commentary", "final_answer"]);
    delete_conversation(&cid).ok();
}

#[test]
fn load_context_omits_phase_field_for_unknown_assistant_phase() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tphase-unknown-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a("a1", "r1", "resp1", None, "done", AssistantStatus::Done),
    })
    .expect("assistant");

    let ctx = load_context_for_request(&cid).expect("ctx");
    let assistant_item = ctx
        .input_items
        .iter()
        .find(|item| item.get("role").and_then(|v| v.as_str()) == Some("assistant"))
        .expect("assistant input item");
    assert!(assistant_item.get("phase").is_none());
    delete_conversation(&cid).ok();
}

#[test]
fn load_context_includes_tool_calls_with_outputs() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tt-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "r1", "calc"),
    })
    .expect("u");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t1",
            "r1",
            "c1",
            "calc",
            json!({"e":"1+1"}),
            Some(json!({"ok":true})),
            ToolStatus::Done,
        ),
    })
    .expect("t");
    let ctx = load_context_for_request(&cid).expect("ctx");
    assert!(ctx
        .input_items
        .iter()
        .any(|i| i.get("call_id").and_then(|v| v.as_str()) == Some("c1")
            && i.get("type").and_then(|v| v.as_str()) == Some("function_call")
            && i.get("arguments").and_then(|v| v.as_str()) == Some("{\"e\":\"1+1\"}")));
    assert!(ctx
        .input_items
        .iter()
        .any(|i| i.get("call_id").and_then(|v| v.as_str()) == Some("c1")
            && i.get("type").and_then(|v| v.as_str()) == Some("function_call_output")));
    delete_conversation(&cid).ok();
}

#[test]
fn update_line_preserves_existing_tool_args_when_exec_event_omits_them() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tmerge-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t1",
            "r1",
            "call_1",
            "calc",
            json!({"expression":"2+2"}),
            None,
            ToolStatus::Pending,
        ),
    })
    .expect("pending");
    update_line(UpdateLineInput {
        conversation_id: cid.clone(),
        line_id: "t1".into(),
        line: t(
            "t1",
            "r1",
            "call_1",
            "calc",
            serde_json::Value::Null,
            Some(json!({"ok":true,"result":4})),
            ToolStatus::Done,
        ),
    })
    .expect("done");

    let ctx = load_context_for_request(&cid).expect("ctx");
    let args = ctx
        .input_items
        .iter()
        .find(|i| {
            i.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && i.get("call_id").and_then(|v| v.as_str()) == Some("call_1")
        })
        .and_then(|item| item.get("arguments"))
        .and_then(|v| v.as_str());
    assert_eq!(args, Some("{\"expression\":\"2+2\"}"));
    delete_conversation(&cid).ok();
}

#[test]
fn update_line_refreshes_summary_metadata_after_streaming_rewrite() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tsummary-update-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "r1", "original user"),
    })
    .expect("user");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::FinalAnswer),
            "old assistant preview",
            AssistantStatus::Done,
        ),
    })
    .expect("assistant");

    update_line(UpdateLineInput {
        conversation_id: cid.clone(),
        line_id: "a1".into(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::FinalAnswer),
            "new assistant preview",
            AssistantStatus::Done,
        ),
    })
    .expect("update");

    let summaries = list_conversations().expect("list conversations");
    let summary = summaries
        .into_iter()
        .find(|item| item.conversation_id == cid)
        .expect("conversation summary");
    assert_eq!(summary.message_count, 2);
    assert_eq!(summary.last_message_preview, "new assistant preview");

    delete_conversation(&cid).ok();
}

#[test]
fn commentary_is_excluded_from_summary_metadata() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tsummary-commentary-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "r1", "需要修一个旁白问题"),
    })
    .expect("user");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::Commentary),
            "我先检查一下前后端链路。",
            AssistantStatus::Done,
        ),
    })
    .expect("commentary");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a2",
            "r1",
            "resp1",
            Some(AssistantPhase::FinalAnswer),
            "已经定位到问题并完成第一轮修复。",
            AssistantStatus::Done,
        ),
    })
    .expect("final");

    let summary = list_conversations()
        .expect("list conversations")
        .into_iter()
        .find(|item| item.conversation_id == cid)
        .expect("conversation summary");
    assert_eq!(summary.message_count, 2);
    assert_eq!(
        summary.last_message_preview,
        "已经定位到问题并完成第一轮修复。"
    );

    delete_conversation(&cid).ok();
}

#[test]
fn summary_refresh_rebuild_still_excludes_commentary_preview() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("trefresh-commentary-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "r1", "请继续"),
    })
    .expect("user");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::Commentary),
            "我先看一下代码结构。",
            AssistantStatus::Done,
        ),
    })
    .expect("commentary");

    let detail = load_conversation(&cid).expect("load").expect("detail");
    assert_eq!(detail.lines.len(), 2);

    let summary = list_conversations()
        .expect("list conversations")
        .into_iter()
        .find(|item| item.conversation_id == cid)
        .expect("conversation summary");
    assert_eq!(summary.message_count, 1);
    assert_eq!(summary.last_message_preview, "请继续");

    delete_conversation(&cid).ok();
}

#[test]
fn duplicate_append_is_skipped_with_cached_line_ids() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tdup-cache-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");

    let first_line = u("u-dup", "r1", "hello once");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: first_line.clone(),
    })
    .expect("first append");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: first_line,
    })
    .expect("duplicate append");

    let detail = load_conversation(&cid).expect("load").expect("detail");
    assert_eq!(detail.lines.len(), 1);

    delete_conversation(&cid).ok();
}

#[test]
fn delete_conversation_clears_line_id_cache_for_recreated_session() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tcache-reset-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u-reset", "r1", "first life"),
    })
    .expect("append");
    assert!(delete_conversation(&cid).expect("delete"));

    ensure_conversation(&cid).expect("recreate");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u-reset", "r2", "second life"),
    })
    .expect("append after recreate");

    let detail = load_conversation(&cid).expect("load").expect("detail");
    assert_eq!(detail.lines.len(), 1);
    let text = match &detail.lines[0] {
        ConversationLine::User(line) => line.text.as_str(),
        other => panic!("expected user line after recreate, got {:?}", other),
    };
    assert_eq!(text, "second life");

    delete_conversation(&cid).ok();
}

#[test]
fn delete_conversation_clears_cached_summary_for_recreated_session() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tsummary-reset-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    rename_conversation(&cid, "旧标题").expect("rename");

    let initial_summary = list_conversations()
        .expect("list before delete")
        .into_iter()
        .find(|item| item.conversation_id == cid)
        .expect("initial summary");
    assert_eq!(initial_summary.title, "旧标题");

    assert!(delete_conversation(&cid).expect("delete"));
    ensure_conversation(&cid).expect("recreate");

    let recreated_summary = list_conversations()
        .expect("list after recreate")
        .into_iter()
        .find(|item| item.conversation_id == cid)
        .expect("recreated summary");
    assert_eq!(
        recreated_summary.title,
        crate::conversation_store::types::DEFAULT_CONVERSATION_TITLE
    );

    delete_conversation(&cid).ok();
}

#[test]
fn load_context_filters_orphan_tool_calls() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("to-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t1",
            "r1",
            "c_ok",
            "a",
            json!({}),
            Some(json!({"ok":true})),
            ToolStatus::Done,
        ),
    })
    .expect("ok");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t2",
            "r1",
            "c_orphan",
            "b",
            json!({}),
            None,
            ToolStatus::Pending,
        ),
    })
    .expect("orphan");
    let ctx = load_context_for_request(&cid).expect("ctx");
    assert!(ctx
        .input_items
        .iter()
        .any(|i| i.get("call_id").and_then(|v| v.as_str()) == Some("c_ok")));
    assert!(!ctx
        .input_items
        .iter()
        .any(|i| i.get("call_id").and_then(|v| v.as_str()) == Some("c_orphan")));
    delete_conversation(&cid).ok();
}

#[test]
fn load_context_truncates_without_splitting_tool_pairs() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("ttrunc-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    for i in 0..260 {
        append_line(AppendLineInput {
            conversation_id: cid.clone(),
            line: u(&format!("u{i}"), "r1", &format!("m{i}")),
        })
        .expect("u");
    }
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "ttail",
            "r1",
            "call_tail",
            "x",
            json!({}),
            Some(json!({"ok":true})),
            ToolStatus::Done,
        ),
    })
    .expect("t");
    let ctx = load_context_for_request(&cid).expect("ctx");
    assert!(ctx.input_items.len() <= TEST_MAX_CONTEXT_ITEMS_PER_REQUEST);
    assert!(ctx
        .input_items
        .iter()
        .any(|i| i.get("call_id").and_then(|v| v.as_str()) == Some("call_tail")));
    delete_conversation(&cid).ok();
}

#[test]
fn build_recovery_note_for_unfinished_turn() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("trec-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "req-rec", "go"),
    })
    .expect("u");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t1",
            "req-rec",
            "call_rec_1",
            "demo",
            json!({"x":1}),
            None,
            ToolStatus::Pending,
        ),
    })
    .expect("t");
    let note = build_recovery_developer_note(&cid)
        .expect("note")
        .expect("some");
    let txt = note
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|p| p.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(txt.contains("RECOVERY_CONTEXT"));
    assert!(txt.contains("req-rec"));
    assert!(txt.contains("call_rec_1"));
    delete_conversation(&cid).ok();
}

#[test]
fn load_conversation_returns_all_lines() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tld-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "r1", "hi"),
    })
    .expect("u");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t1",
            "r1",
            "c1",
            "s",
            json!({"q":"x"}),
            Some(json!({"ok":true})),
            ToolStatus::Done,
        ),
    })
    .expect("t");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "r1",
            "resp1",
            Some(AssistantPhase::FinalAnswer),
            "done",
            AssistantStatus::Done,
        ),
    })
    .expect("a");
    let d = load_conversation(&cid).expect("load").expect("detail");
    assert_eq!(d.lines.len(), 3);
    assert!(d
        .lines
        .iter()
        .any(|l| matches!(l, ConversationLine::Tool(tl) if tl.call_id == "c1")));
    delete_conversation(&cid).ok();
}

#[test]
fn fault_injection_recovery_clears_after_completion() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tfault-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");

    // crash mid-turn: user + pending tool
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "req-f", "call tool"),
    })
    .expect("u");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: t(
            "t1",
            "req-f",
            "call_f_1",
            "s",
            json!({"q":"x"}),
            None,
            ToolStatus::Pending,
        ),
    })
    .expect("t");

    let note = build_recovery_developer_note(&cid)
        .expect("note")
        .expect("some");
    let txt = note
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|p| p.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(txt.contains("call_f_1"));
    assert!(txt.contains("unresolved"));

    // resume: update tool → done, then assistant
    update_line(UpdateLineInput {
        conversation_id: cid.clone(),
        line_id: "t1".into(),
        line: t(
            "t1",
            "req-f",
            "call_f_1",
            "s",
            json!({"q":"x"}),
            Some(json!({"ok":true})),
            ToolStatus::Done,
        ),
    })
    .expect("upd");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a(
            "a1",
            "req-f",
            "resp-f",
            Some(AssistantPhase::FinalAnswer),
            "found",
            AssistantStatus::Done,
        ),
    })
    .expect("a");

    let note2 = build_recovery_developer_note(&cid).expect("note2");
    assert!(
        note2.is_none(),
        "recovery note should be None after completion"
    );

    delete_conversation(&cid).ok();
}

#[test]
fn recovery_treats_unknown_assistant_phase_as_completed_answer() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("trecovery-unknown-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: u("u1", "req-1", "hello"),
    })
    .expect("user");
    append_line(AppendLineInput {
        conversation_id: cid.clone(),
        line: a("a1", "req-1", "resp-1", None, "done", AssistantStatus::Done),
    })
    .expect("assistant");

    let note = build_recovery_developer_note(&cid).expect("recovery note");
    assert!(
        note.is_none(),
        "unknown assistant phase should count as completed"
    );
    delete_conversation(&cid).ok();
}

#[test]
fn conversation_dynamic_tools_round_trip_through_metadata() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tdyntools-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");

    update_conversation_dynamic_tools(
        &cid,
        vec![ConversationDynamicTool {
            name: "math_alias".to_string(),
            display_name: None,
            description: "Alias".to_string(),
            icon: None,
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": { "type": "string" }
                }
            }),
            binding: ConversationDynamicToolBinding::Native {
                tool: "calculator".to_string(),
            },
        }],
    )
    .expect("persist dynamic tools");

    let loaded = load_conversation_dynamic_tools(&cid).expect("load dynamic tools");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "math_alias");
    assert_eq!(
        loaded[0].binding,
        ConversationDynamicToolBinding::Native {
            tool: "calculator".to_string()
        }
    );

    update_conversation_dynamic_tools(&cid, Vec::new()).expect("clear dynamic tools");
    assert!(load_conversation_dynamic_tools(&cid)
        .expect("reload dynamic tools")
        .is_empty());
}

#[test]
fn conversation_mounted_tool_sources_round_trip_through_metadata() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tmountedtools-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");

    update_conversation_mounted_tool_sources(
        &cid,
        vec![ConversationMountedToolSource {
            source_id: "openai_docs".to_string(),
            source_type: "mcp".to_string(),
            tools: vec![ConversationMountedToolDefinition {
                tool_name: "search_openai_docs".to_string(),
                display_name: "Search Openai Docs".to_string(),
                description: "Search docs".to_string(),
                icon: Some("LayoutGrid".to_string()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    }
                }),
            }],
        }],
    )
    .expect("persist mounted tool sources");

    let loaded = load_conversation_mounted_tool_sources(&cid).expect("load mounted tool sources");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].source_id, "openai_docs");
    assert_eq!(loaded[0].source_type, "mcp");
    assert_eq!(loaded[0].tools.len(), 1);
    assert_eq!(loaded[0].tools[0].tool_name, "search_openai_docs");

    update_conversation_mounted_tool_sources(&cid, Vec::new()).expect("clear mounted tool sources");
    assert!(load_conversation_mounted_tool_sources(&cid)
        .expect("reload mounted tool sources")
        .is_empty());
}

#[test]
fn conversation_mounted_mcp_servers_legacy_fallback() {
    use super::paths::conversation_metadata_path;
    use super::file_io::{read_conversation_meta, write_conversation_metadata};

    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tlegacyfallback-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");

    // Manually insert legacy format under old key in metadata
    let metadata_path = conversation_metadata_path(&cid).expect("metadata path");
    let mut meta = read_conversation_meta(&metadata_path).expect("read meta").expect("meta exists");
    let legacy_data = json!([
        {
            "serverId": "openai_docs",
            "tools": [
                {
                    "toolName": "search_openai_docs",
                    "displayName": "Search Openai Docs",
                    "description": "Search docs",
                    "icon": "LayoutGrid",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        }
                    }
                }
            ]
        }
    ]);
    meta.metadata.insert("mounted_mcp_servers".to_string(), legacy_data);
    write_conversation_metadata(&metadata_path, &meta).expect("write legacy meta");

    // Load using generic loader and assert mapped fields
    let loaded = load_conversation_mounted_tool_sources(&cid).expect("load mounted tool sources");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].source_id, "openai_docs");
    assert_eq!(loaded[0].source_type, "mcp");
    assert_eq!(loaded[0].tools.len(), 1);
    assert_eq!(loaded[0].tools[0].tool_name, "search_openai_docs");
}

#[test]
fn concurrent_appends_preserve_all_lines_for_same_conversation() {
    let _g = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _h = setup_test_home();
    let cid = format!("tconcurrent-{}", Uuid::new_v4());
    ensure_conversation(&cid).expect("ensure");

    let mut handles = Vec::new();
    for idx in 0..12 {
        let conversation_id = cid.clone();
        handles.push(std::thread::spawn(move || {
            append_line(AppendLineInput {
                conversation_id,
                line: u(
                    &format!("u{idx}"),
                    &format!("req-{idx}"),
                    &format!("message {idx}"),
                ),
            })
        }));
    }

    for handle in handles {
        handle.join().expect("join").expect("append");
    }

    let detail = load_conversation(&cid).expect("load").expect("detail");
    let user_count = detail
        .lines
        .iter()
        .filter(|line| matches!(line, ConversationLine::User(_)))
        .count();
    assert_eq!(user_count, 12);

    delete_conversation(&cid).ok();
}
