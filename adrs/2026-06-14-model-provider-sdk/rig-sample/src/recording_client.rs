use bytes::Bytes;
use rig_core::http_client::{
    self, HeaderValue, HttpClientExt, LazyBody, MultipartForm, ReqwestClient, StreamingResponse,
};
use rig_core::wasm_compat::WasmCompatSend;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CassetteEntry {
    pub request_method: String,
    pub request_path: String,
    pub request_body: String,
    pub response_status: u16,
    pub response_body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cassette {
    pub entries: Vec<CassetteEntry>,
}

#[derive(Clone)]
pub struct RecordingClient {
    inner: ReqwestClient,
    cassette: Arc<Mutex<Cassette>>,
    recording: bool,
    cassette_path: String,
    replay_index: Arc<Mutex<usize>>,
    read_cassette: bool,
}

impl RecordingClient {
    pub fn new(recording: bool, cassette_path: String) -> Self {
        Self {
            inner: ReqwestClient::new(),
            cassette: Arc::new(Mutex::new(Cassette { entries: Vec::new() })),
            recording,
            cassette_path,
            replay_index: Arc::new(Mutex::new(0)),
            read_cassette: !recording,
        }
    }

    pub fn new_passthrough(recording: bool, cassette_path: String) -> Self {
        Self {
            inner: ReqwestClient::new(),
            cassette: Arc::new(Mutex::new(Cassette { entries: Vec::new() })),
            recording,
            cassette_path,
            replay_index: Arc::new(Mutex::new(0)),
            read_cassette: false,
        }
    }
}

impl Default for RecordingClient {
    fn default() -> Self {
        Self::new_passthrough(false, "cassette.json".to_string())
    }
}

impl std::fmt::Debug for RecordingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingClient")
            .field("recording", &self.recording)
            .field("cassette_path", &self.cassette_path)
            .finish()
    }
}

impl Drop for RecordingClient {
    fn drop(&mut self) {
        if self.recording {
            let cassette = self.cassette.lock().unwrap();
            if let Ok(json) = serde_json::to_string_pretty(&*cassette) {
                let _ = std::fs::write(&self.cassette_path, json);
            }
        }
    }
}

impl HttpClientExt for RecordingClient {
    fn send<T, U>(
        &self,
        req: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let (parts, body) = req.into_parts();
        let body_bytes: Bytes = body.into();
        let self_clone = self.clone();
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();

        Box::pin(async move {
            if self_clone.read_cassette {
                let mut cassette = self_clone.cassette.lock().unwrap();
                if cassette.entries.is_empty() {
                    let json = std::fs::read_to_string(&self_clone.cassette_path)
                        .map_err(|e| http_client::Error::Instance(e.into()))?;
                    *cassette = serde_json::from_str(&json)
                        .map_err(|e| http_client::Error::Instance(e.into()))?;
                }
                let idx = {
                    let mut idx_lock = self_clone.replay_index.lock().unwrap();
                    let i = *idx_lock;
                    *idx_lock += 1;
                    i
                };

                let entry = cassette.entries.get(idx).ok_or_else(|| {
                    http_client::Error::Instance(
                        format!(
                            "cassette exhausted at index {}, only {} entries available. \
                             Request was: {} {} body={}",
                            idx,
                            cassette.entries.len(),
                            parts.method,
                            parts.uri,
                            body_str
                        )
                        .into(),
                    )
                })?;

                let status = http::StatusCode::from_u16(entry.response_status).unwrap();
                let resp_bytes: Bytes = Bytes::from(entry.response_body.clone());

                let mut res = http::Response::builder().status(status);
                if let Some(hs) = res.headers_mut() {
                    hs.insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                }

                let body: LazyBody<U> = Box::pin(async move { Ok(U::from(resp_bytes)) });

                return res.body(body).map_err(http_client::Error::Protocol);
            }

            let reqwest_req = self_clone
                .inner
                .request(parts.method.clone(), parts.uri.to_string())
                .headers(parts.headers.clone())
                .body(body_bytes.clone());

            let response = reqwest_req
                .send()
                .await
                .map_err(|e| http_client::Error::Instance(e.into()))?;

            let status = response.status();
            let resp_bytes = response
                .bytes()
                .await
                .map_err(|e| http_client::Error::Instance(e.into()))?;

            if self_clone.recording {
                let mut cassette = self_clone.cassette.lock().unwrap();
                cassette.entries.push(CassetteEntry {
                    request_method: parts.method.to_string(),
                    request_path: parts.uri.to_string(),
                    request_body: body_str,
                    response_status: status.as_u16(),
                    response_body: String::from_utf8_lossy(&resp_bytes).to_string(),
                });
            }

            let mut res = http::Response::builder().status(status);
            if let Some(hs) = res.headers_mut() {
                hs.insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
            }

            let body: LazyBody<U> = Box::pin(async move { Ok(U::from(resp_bytes)) });

            res.body(body).map_err(http_client::Error::Protocol)
        })
    }

    fn send_multipart<U>(
        &self,
        _req: http::Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        Box::pin(async {
            Err(http_client::Error::Instance(
                "send_multipart not implemented".to_string().into(),
            ))
        })
    }

    fn send_streaming<T>(
        &self,
        _req: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        Box::pin(async {
            Err(http_client::Error::Instance(
                "send_streaming not implemented".to_string().into(),
            ))
        })
    }
}
