use serde_json::Value;

pub trait SseProcessor: Send + Sync {
    fn translate_event(&self, event_type: &str, data: &Value, response_id: &mut Option<String>) -> Option<String>;
    fn extract_response(&self, body: &[u8]) -> Option<Value>;
}
