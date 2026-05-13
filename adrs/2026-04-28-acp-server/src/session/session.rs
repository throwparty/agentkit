use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Represents a message in a session's conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub timestamp: u64,
}

/// Represents a session with its message history and state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub title: String,
    pub mode: Option<String>,
    pub messages: VecDeque<Message>,
    pub created_at: u64,
    pub updated_at: u64,
    pub active: bool,
    pub transport: String,
}

impl Session {
    pub fn new(id: String, transport: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Self {
            id,
            cwd: String::new(),
            title: String::new(),
            mode: None,
            messages: VecDeque::new(),
            created_at: now,
            updated_at: now,
            active: true,
            transport,
        }
    }

    pub fn add_message(&mut self, role: String, content: String) {
        self.messages.push_back(Message {
            role,
            content,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        });
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
    }

    pub fn close(&mut self) {
        self.active = false;
    }

    #[allow(dead_code)]
    pub fn get_messages(&self) -> &VecDeque<Message> {
        &self.messages
    }
}
