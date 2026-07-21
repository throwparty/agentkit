use crate::config::BillingModel;
use crate::domain::conversation::ConversationHandler;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct OpenAiConversation;

impl ConversationHandler for OpenAiConversation {
    fn prepare_request(&self, body: Value, _billing: &BillingModel) -> Result<Value, String> {
        Ok(body)
    }

    fn prepare_response(&self, body: Value, _billing: &BillingModel) -> Result<Value, String> {
        Ok(body)
    }
}
