use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("Invalid session: {0}")]
    InvalidSession(String),

    #[error("Protocol error: {0}")]
    Protocol(String),
}

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl AcpError {
    pub fn to_rpc_error(&self) -> RpcError {
        match self {
            AcpError::InvalidSession(msg) => RpcError {
                code: -32602,
                message: msg.clone(),
            },
            AcpError::Protocol(msg) => RpcError {
                code: -32603,
                message: msg.clone(),
            },
        }
    }


}
