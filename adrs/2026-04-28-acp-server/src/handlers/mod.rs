use std::io::Write;
use serde_json::{json, Value};
use uuid::Uuid;
use crate::error::AcpError;
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::session::session::Session;
use crate::session::store::SessionStore;

#[derive(Clone)]
pub struct Router {
    session_store: SessionStore,
}

impl Router {
    pub fn new(session_store: SessionStore) -> Self {
        Self { session_store }
    }

    pub async fn route(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "ping" => self.handle_ping(request).await,
            "session/new" => self.handle_session_new(request).await,
            "session/load" => self.handle_session_load(request).await,
            "session/resume" => self.handle_session_resume(request).await,
            "session/list" => self.handle_session_list(request).await,
            "session/close" => self.handle_session_close(request).await,
            "session/set_mode" => self.handle_session_set_mode(request).await,
            "session/prompt" => self.handle_session_prompt(request).await,
            "session/cancel" => self.handle_session_cancel(request).await,
            other => {
                let rpc_err = AcpError::Protocol(format!("Unknown method: {other}")).to_rpc_error();
                request.to_error_response(rpc_err.code, rpc_err.message)
            }
        }
    }

    fn get_session_id(request: &JsonRpcRequest) -> Result<String, (i64, String)> {
        let map = request.params.as_ref()
            .and_then(|p| p.as_object())
            .ok_or((-32602, "No params".into()))?;
        map.get("sessionId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or((-32602, "Missing sessionId".into()))
    }

    async fn handle_initialize(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let client_version = request
            .params
            .as_ref()
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        if client_version != 1 {
            return request.to_error_response(
                -32602,
                format!("Unsupported protocol version: {}. Server supports: 1", client_version),
            );
        }

        let result = json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {
                    "image": false,
                    "audio": false,
                    "embeddedContext": false
                },
                "mcpCapabilities": {
                    "http": false,
                    "sse": false
                },
                "sessionCapabilities": {
                    "list": {},
                    "close": {},
                    "resume": {}
                }
            },
            "agentInfo": {
                "name": "acp-server",
                "title": "ACP Server Harness",
                "version": "0.1.0"
            },
            "authMethods": []
        });
        request.to_response(result)
    }

    async fn handle_ping(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        req.to_response(json!({"ping": "pong"}))
    }

    async fn handle_session_new(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let session_id = Uuid::new_v4().to_string();
        let mut session = Session::new(session_id.clone(), "".to_string());

        if let Some(Value::Object(map)) = &request.params {
            if let Some(cwd) = map.get("cwd").and_then(|v| v.as_str()) {
                session.cwd = cwd.to_string();
            }
            if let Some(title) = map.get("title").and_then(|v| v.as_str()) {
                session.title = title.to_string();
            }
        }

        match self.session_store.create(session).await {
            Ok(()) => request.to_response(json!({
                "sessionId": session_id,
                "modes": {
                    "current": null,
                    "available": []
                }
            })),
            Err(e) => request.to_error_response(-32602, e),
        }
    }

    async fn replay_dummy_history(session_id: &str, store: &SessionStore) {
        let dummy_updates = vec![
            ("user", "hello"),
            ("agent", "hello! how can i help you?"),
            ("user", "what's the weather?"),
            ("agent", "i'm an echo server, so: what's the weather?"),
        ];

        for (role, text) in &dummy_updates {
            let kind = if *role == "user" { "user_message_chunk" } else { "agent_message_chunk" };
            let notification = json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": kind,
                        "content": {
                            "type": "text",
                            "text": text
                        }
                    }
                }
            });
            println!("{}", serde_json::to_string(&notification).unwrap());
            let _ = std::io::stdout().flush();
        }

        // Persist dummy messages in-memory so they compound across
        // resumes/loads within the same process lifetime
        if store.get(session_id).await.is_ok() {
            for (role, text) in &dummy_updates {
                let _ = store.add_message(session_id, role.to_string(), text.to_string()).await;
            }
        } else {
            let mut session = Session::new(session_id.to_string(), "".to_string());
            for (role, text) in &dummy_updates {
                session.add_message(role.to_string(), text.to_string());
            }
            let _ = store.create(session).await;
        }
    }

    async fn handle_session_load(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let session_id = match Self::get_session_id(request) {
            Ok(id) => id,
            Err((code, msg)) => return request.to_error_response(code, msg),
        };

        Self::replay_dummy_history(&session_id, &self.session_store).await;
        request.to_response(json!({}))
    }

    async fn handle_session_resume(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let session_id = match Self::get_session_id(request) {
            Ok(id) => id,
            Err((code, msg)) => return request.to_error_response(code, msg),
        };

        Self::replay_dummy_history(&session_id, &self.session_store).await;
        request.to_response(json!({}))
    }

    async fn handle_session_list(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let sessions = self.session_store.list().await;
        let session_list: Vec<Value> = sessions.iter().map(|s| {
            let has_errors = s.messages.iter().any(|m| m.role == "error");
            json!({
                "sessionId": s.id,
                "cwd": s.cwd,
                "title": s.title,
                "updatedAt": s.updated_at,
                "_meta": {
                    "messageCount": s.messages.len(),
                    "hasErrors": has_errors
                }
            })
        }).collect();

        request.to_response(json!({"sessions": session_list}))
    }

    async fn handle_session_close(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let session_id = match Self::get_session_id(request) {
            Ok(id) => id,
            Err((code, msg)) => return request.to_error_response(code, msg),
        };

        match self.session_store.close(&session_id).await {
            Ok(()) => request.to_response(json!({})),
            Err(e) => request.to_error_response(-32602, e),
        }
    }

    async fn handle_session_set_mode(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let session_id = match Self::get_session_id(request) {
            Ok(id) => id,
            Err((code, msg)) => return request.to_error_response(code, msg),
        };

        let mode = match request.params.as_ref()
            .and_then(|p| p.get("mode"))
            .and_then(|v| v.as_str())
        {
            Some(m) => m.to_string(),
            None => return request.to_error_response(-32602, "Missing mode".into()),
        };

        match self.session_store.set_mode(&session_id, mode).await {
            Ok(()) => request.to_response(json!({})),
            Err(e) => request.to_error_response(-32602, e),
        }
    }

    async fn handle_session_prompt(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let session_id = match Self::get_session_id(request) {
            Ok(id) => id,
            Err((code, msg)) => return request.to_error_response(code, msg),
        };

        let params_map = match &request.params {
            Some(Value::Object(map)) => map.clone(),
            _ => return request.to_error_response(-32602, "No params".into()),
        };

        let prompt = params_map.get("prompt")
            .or_else(|| params_map.get("message"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut full_text = String::new();
        for block in &prompt {
            if let Some(obj) = block.as_object() {
                let content_val = obj.get("text").or_else(|| obj.get("content"));
                if let Some(text) = content_val.and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        if !full_text.is_empty() {
                            full_text.push('\n');
                        }
                        full_text.push_str(text);
                    }
                }
            }
        }

        if full_text.is_empty() {
            return request.to_error_response(-32602, "Empty prompt".into());
        }

        // Send session/update notification with agent_message_chunk
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id.clone(),
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {
                        "type": "text",
                        "text": full_text.clone()
                    }
                }
            }
        });
        println!("{}", serde_json::to_string(&notification).unwrap());
        std::io::stdout().flush().unwrap();

        match self.session_store.add_message(&session_id, "user".into(), full_text).await {
            Ok(()) => request.to_response(json!({"stopReason": "end_turn"})),
            Err(e) => request.to_error_response(-32602, e),
        }
    }

    async fn handle_session_cancel(&self, _request: &JsonRpcRequest) -> JsonRpcResponse {
        // session/cancel is a notification (no id per JSON-RPC 2.0)
        // In a notification, we do not return a response
        // The transport layer will still serialize this, but the client
        // will ignore it since there's no matching pending request id
        JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: None,
            result: None,
            error: None,
        }
    }
}
