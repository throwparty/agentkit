use std::io::{self, BufRead};
use serde_json::json;

use crate::jsonrpc::JsonRpcRequest;
use crate::handlers::Router;
use crate::session::store::SessionStore;

pub async fn run_stdio() {
    let store = SessionStore::new();
    let router = Router::new(store);

    let stdin = io::stdin();

    for line in stdin.lock().lines() {
        match line {
            Ok(line) => {
                let request: JsonRpcRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(e) => {
                        eprintln!("Failed to parse JSON: {}", e);
                        let error_response = json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32700,
                                "message": "Parse error"
                            },
                            "id": null
                        });
                        println!("{}", serde_json::to_string(&error_response).unwrap());
                        continue;
                    }
                };

                let response = router.route(&request).await;

                let response_json = serde_json::to_string(&response).unwrap();
                println!("{}", response_json);
            }
            Err(e) => {
                eprintln!("Error reading line: {}", e);
                break;
            }
        }
    }
}
