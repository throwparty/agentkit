use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::session::Session;

/// In-memory session store backed by RwLock-protected HashMap
#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new session and store it
    pub async fn create(&self, session: Session) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&session.id) {
            return Err(format!("Session {} already exists", session.id));
        }
        sessions.insert(session.id.clone(), session);
        Ok(())
    }

    /// Get a session by ID
    pub async fn get(&self, id: &str) -> Result<Session, String> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned().ok_or_else(|| format!("Session {} not found", id))
    }

    /// List all sessions
    pub async fn list(&self) -> Vec<Session> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }

    /// Close a session
    pub async fn close(&self, id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id).ok_or_else(|| format!("Session {} not found", id))?;
        session.close();
        Ok(())
    }

    /// Add a message to a session
    pub async fn add_message(&self, id: &str, role: String, content: String) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id).ok_or_else(|| format!("Session {} not found", id))?;
        session.add_message(role, content);
        Ok(())
    }

    /// Set the mode for a session
    pub async fn set_mode(&self, id: &str, mode: String) -> Result<(), String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(id).ok_or_else(|| format!("Session {} not found", id))?;
        session.mode = Some(mode);
        Ok(())
    }

    /// Clear all sessions
    #[allow(dead_code)]
    pub async fn clear(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.clear();
    }
}
