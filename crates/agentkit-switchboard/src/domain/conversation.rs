use crate::config::BillingModel;
use serde_json::Value;

pub trait ConversationHandler: Send + Sync {
    fn prepare_request(&self, body: Value, billing: &BillingModel) -> Result<Value, String>;
    fn prepare_response(&self, body: Value, billing: &BillingModel) -> Result<Value, String>;
}
