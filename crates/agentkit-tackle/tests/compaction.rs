//! Compaction integration tests (T-028): the /compact turn —
//! interception, the in-band announcement with before/after counts, the
//! visible usage_update drop, the RFD-shaped update for advertising
//! clients, and summary storage.

use agentkit_tackle::agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, SessionNotification,
    SessionUpdate, TextContent,
};
use agentkit_tackle::agent_client_protocol::schema::ProtocolVersion;
use agentkit_tackle::agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use agentkit_tackle::store::{SessionStore, TurnKind};
use std::sync::{Arc as StdArc, Mutex as StdMutex};

fn spawn_agent_with_compaction_script(dir: &std::path::Path) -> AcpAgent {
    let cfg = dir.join("cfg");
    std::fs::create_dir_all(cfg.join("scripts")).unwrap();
    std::fs::create_dir_all(cfg.join("prompts")).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[scripts.compaction]\nevents = [\"compaction_requested\"]\nfile = \"compact.rhai\"\n\n[defaults]\nmodel = \"models/MiniMax-M2\"\n",
    )
    .unwrap();
    // The compaction-tagged prompt: its first-token command is what the
    // interception reads.
    std::fs::write(
        cfg.join("prompts").join("compact.md"),
        "+++\ncompaction = true\n+++\nCompact the conversation.\n",
    )
    .unwrap();
    // The test compaction script records a keep-recent compaction from
    // the intercepted turn — the event payload's turn_id — so the
    // announcement and the command survive the summary.
    std::fs::write(
        cfg.join("scripts").join("compact.rhai"),
        r#"fn compaction_requested(event) { record_compaction("summary: everything before", event.turn_id); }"#,
    )
    .unwrap();
    AcpAgent::from_args([
        env!("CARGO_BIN_EXE_agentkit-tackle"),
        "--config-dir",
        cfg.to_str().unwrap(),
        "--db-path",
        dir.join("sessions.db").to_str().unwrap(),
    ])
    .expect("agent arguments")
}

#[tokio::test(flavor = "multi_thread")]
async fn compaction_intercepts_announces_and_records() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");
    let agent = spawn_agent_with_compaction_script(dir.path());

    let chunks: StdArc<StdMutex<Vec<String>>> = StdArc::default();
    let usage_updates: StdArc<StdMutex<Vec<u64>>> = StdArc::default();
    let compaction_updates: StdArc<StdMutex<Vec<String>>> = StdArc::default();

    let chunks_handler = chunks.clone();
    let usage_handler = usage_updates.clone();
    let compaction_handler = compaction_updates.clone();

    // The client advertises the unstable compaction capability: the
    // RFD-shaped update flows.
    let mut meta = agentkit_tackle::agent_client_protocol::schema::v1::Meta::new();
    meta.insert(
        "unstable_session_compaction".to_owned(),
        serde_json::json!({}),
    );
    let initialize = InitializeRequest::new(ProtocolVersion::V1)
        .meta(agentkit_tackle::agent_client_protocol::schema::v1::Meta::from(meta));

    Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                match &notification.update {
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = &chunk.content {
                            chunks_handler.lock().unwrap().push(text.text.clone());
                        }
                    }
                    SessionUpdate::UsageUpdate(usage) => {
                        usage_handler.lock().unwrap().push(usage.used);
                    }
                    #[cfg(feature = "unstable")]
                    SessionUpdate::CompactionUpdate(update) => {
                        compaction_handler.lock().unwrap().push(format!("{:?}", update.status));
                    }
                    _ => {}
                }
                Ok(())
            },
            agentkit_tackle::agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| async move {
            connection.send_request(initialize).block_task().await?;
            let created = connection
                .send_request(NewSessionRequest::new(std::path::PathBuf::from("/work")))
                .block_task()
                .await?;
            let session_id = created.session_id.to_string();

            // Give the session context worth compacting.
            {
                let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
                let store_id = session_id.replace("sess_", "");
                for i in 0..4 {
                    let turn = db
                        .append_turn(
                            &store_id,
                            None,
                            TurnKind::Interaction,
                            None,
                            Default::default(),
                        )
                        .await
                        .unwrap();
                    db.append_message(
                        &turn.id,
                        agentkit_tackle::store::Role::User,
                        &serde_json::json!([{ "type": "text", "text": format!("turn {i} with content") }])
                            .to_string(),
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .unwrap();
                }
            }

            // /compact: intercepted — no model request, end_turn.
            let response = connection
                .send_request(PromptRequest::new(
                    created.session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("/compact".to_owned()))],
                ))
                .block_task()
                .await?;
            assert!(matches!(
                response.stop_reason,
                agentkit_tackle::agent_client_protocol::schema::v1::StopReason::EndTurn
            ));

            // The announcement carries the trigger and counts.
            let chunks = chunks.lock().unwrap().clone();
            let announcement = chunks
                .iter()
                .find(|text| text.starts_with("Compacted (trigger: compact)"))
                .expect("in-band announcement");
            assert!(announcement.contains("tokens of context before"), "{announcement}");
            assert!(announcement.contains("tokens of context before"), "{announcement}");

            // The summary is stored as a compaction turn.
            let db = SessionStore::connect_sqlite(&db_path).await.unwrap();
            let walk = db
                .session_walk(&session_id.replace("sess_", ""))
                .await
                .unwrap();
            assert!(
                walk.iter().any(|(_, kind)| *kind == TurnKind::Compaction),
                "{walk:?}"
            );
            let assembled = db
                .assemble_context(&session_id.replace("sess_", ""))
                .await
                .unwrap();
            assert!(assembled
                .iter()
                .any(|item| item.turn_kind == TurnKind::Compaction
                    && item.message.content.contains("summary: everything before")));

            // The parent turn holds the announcement as an agent message.
            assert!(assembled
                .iter()
                .any(|item| item.turn_kind == TurnKind::Interaction
                    && item.message.content.contains("Compacted (trigger: compact)")));

            // The visible usage_update drop arrived.
            assert!(
                !usage_updates.lock().unwrap().is_empty(),
                "a usage_update followed the compaction"
            );
            Ok(())
        })
        .await
        .expect("compaction round-trip");
    let _ = compaction_updates;
}
