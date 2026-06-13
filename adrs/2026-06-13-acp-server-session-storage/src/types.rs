use std::collections::VecDeque;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub title: String,
    pub mode: Option<String>,
    pub prompt_turns: VecDeque<PromptTurn>,
    pub prompt_turn_count: usize,
    pub forked_from_session_id: Option<String>,
    pub fork_point_turn_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub active: bool,
    pub transport: String,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct PromptTurn {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub messages: Vec<Message>,
    pub position: usize,
    pub created_at: u64,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: String,
    pub prompt_turn_id: String,
    pub role: String,
    pub content: String,
    pub position: usize,
    pub created_at: u64,
}
