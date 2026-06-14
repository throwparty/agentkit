use std::collections::HashMap;
use std::sync::RwLock;
use crate::session::{SessionAffinity, SessionError, SessionManager, RoutingEvent};

pub struct MemorySessionManager {
    sessions: RwLock<HashMap<String, SessionAffinity>>,
    token_counts: RwLock<HashMap<String, (u64, u64, u32)>>,
    events: RwLock<Vec<RoutingEvent>>,
}

impl Default for MemorySessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySessionManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            token_counts: RwLock::new(HashMap::new()),
            events: RwLock::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl SessionManager for MemorySessionManager {
    async fn lookup(&self, session_id: &str) -> Result<Option<SessionAffinity>, SessionError> {
        let sessions = self.sessions.read().map_err(|e| SessionError::Database(e.to_string()))?;
        Ok(sessions.get(session_id).cloned())
    }

    async fn assign(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        surface: &str,
    ) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().map_err(|e| SessionError::Database(e.to_string()))?;
        sessions.insert(session_id.to_string(), SessionAffinity {
            session_id: session_id.to_string(),
            provider_identity: provider.to_string(),
            model_name: model.to_string(),
            api_surface: surface.to_string(),
        });
        let mut counts = self.token_counts.write().map_err(|e| SessionError::Database(e.to_string()))?;
        counts.entry(session_id.to_string()).or_insert((0, 0, 0));
        Ok(())
    }

    async fn update_tokens(
        &self,
        session_id: &str,
        input: u64,
        output: u64,
    ) -> Result<(), SessionError> {
        let mut counts = self.token_counts.write().map_err(|e| SessionError::Database(e.to_string()))?;
        let entry = counts.entry(session_id.to_string()).or_insert((0, 0, 0));
        entry.0 += input;
        entry.1 += output;
        entry.2 += 1;
        Ok(())
    }

    async fn increment_switch(
        &self,
        session_id: &str,
        new_provider: &str,
    ) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().map_err(|e| SessionError::Database(e.to_string()))?;
        if let Some(sa) = sessions.get_mut(session_id) {
            sa.provider_identity = new_provider.to_string();
        }
        Ok(())
    }

    async fn insert_routing_event(&self, event: RoutingEvent) -> Result<(), SessionError> {
        let mut events = self.events.write().map_err(|e| SessionError::Database(e.to_string()))?;
        events.push(event);
        Ok(())
    }
}
