use rmcp::{
    handler::server::{
        router::tool::ToolRouter,
        wrapper::Parameters,
    },
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    service::serve_server,
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

use crate::brave;
use crate::kagi;
use crate::cache::Cache;
use crate::config;
use crate::safety;
use crate::search::{EngineInfoOutput, EngineRegistry, SearchRequest};

// ---------------------------------------------------------------------------
// Tool request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchArgs {
    pub query: String,
    pub engine: String,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
    pub region: Option<String>,
}

fn default_page() -> u32 { 1 }
fn default_max_results() -> u32 { 10 }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchResultEntry {
    pub title: String,
    pub link: String,
    pub snippet: String,
    pub position: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchOutput {
    pub results: Vec<SearchResultEntry>,
    pub query: String,
    pub engine: String,
    pub page: u32,
    pub total_pages: u32,
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FetchArgs {
    pub uri: String,
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    #[serde(default = "default_start_index")]
    pub start_index: usize,
    #[serde(default = "default_format")]
    pub format: String,
}

fn default_max_length() -> usize { 8000 }
fn default_start_index() -> usize { 0 }
fn default_format() -> String { "markdown".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchOutput {
    pub content: String,
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub content_length: usize,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnginesResponseOutput {
    pub engines: Vec<EngineInfoOutput>,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

/// Shared state for the MCP server.
pub struct LensServer {
    registry: Arc<Mutex<EngineRegistry>>,
    cache: Arc<Mutex<Cache>>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl LensServer {
    pub fn new(registry: EngineRegistry, cache: Cache) -> Self {
        Self {
            registry: Arc::new(Mutex::new(registry)),
            cache: Arc::new(Mutex::new(cache)),
            tool_router: Self::tool_router(),
        }
    }

    /// Build a `LensServer` from CLI config.
    pub fn from_config(cfg: &config::Config) -> Result<Self, String> {
        let cache_ttl = cfg.parse_cache_ttl();

        let mut registry = EngineRegistry::new();

        if let Some(ref api_key) = cfg.brave_search_api_key {
            registry.register(Box::new(brave::BraveSearchEngine::new(brave::BraveOptions {
                api_key: api_key.clone(),
            })));
        }

        // Register Kagi search engine
        if let Some(ref api_key) = cfg.kagi_search_api_key {
            registry.register(Box::new(kagi::KagiSearchEngine::new(kagi::KagiOptions {
                api_key: api_key.clone(),
            })));
        }

        let cache = Cache::new(cache_ttl);

        Ok(Self::new(registry, cache))
    }
}

// ---------------------------------------------------------------------------
// Tool router — contains tool methods
// ---------------------------------------------------------------------------

#[tool_router]
impl LensServer {
    #[tool(
        name = "search",
        description = "Search the web using a configured search engine"
    )]
    async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Validate query
        if args.query.len() > 400 {
            return Ok(CallToolResult::error(vec![
                Content::text("Query exceeds 400 character limit".to_string()),
            ]));
        }
        if args.query.split_whitespace().count() > 50 {
            return Ok(CallToolResult::error(vec![
                Content::text("Query exceeds 50 word limit".to_string()),
            ]));
        }

        // Build request
        let req = SearchRequest {
            query: args.query,
            engine: args.engine.clone(),
            page: args.page,
            max_results: args.max_results.min(20),
            region: args.region.clone(),
        };

        // Resolve engine
        let registry = self.registry.lock().await;
        let engine = registry.get(&args.engine)
            .ok_or_else(|| {
                McpError::invalid_params(format!("Unknown search engine: {}", args.engine), None)
            })?;

        if !engine.is_configured() {
            let hint = engine.config_hint().unwrap_or_else(|| "Engine is not configured".to_string());
            return Ok(CallToolResult::error(vec![
                Content::text(format!(
                    "Search engine '{}' is not configured: {}",
                    args.engine, hint
                )),
            ]));
        }

        // Execute search
        match engine.search(req).await {
            Ok(response) => {
                let output = SearchOutput {
                    results: response.results.into_iter().map(|r| SearchResultEntry {
                        title: r.title,
                        link: r.link,
                        snippet: r.snippet,
                        position: r.position,
                    }).collect(),
                    query: response.query,
                    engine: response.engine,
                    page: response.page,
                    total_pages: response.total_pages,
                    has_more: response.has_more,
                };
                let content = Content::json(output)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![content]))
            }
            Err(e) => {
                warn!("Search error: {}", e);
                Ok(CallToolResult::error(vec![
                    Content::text(format!("Search failed: {}", e)),
                ]))
            }
        }
    }

    #[tool(
        name = "fetch",
        description = "Fetch a URL and return its content as markdown"
    )]
    async fn fetch(
        &self,
        Parameters(args): Parameters<FetchArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Validate URL for safety
        let url = safety::validate_url(&args.uri)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        // Check cache first
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.get(&url) {
            let output = FetchOutput {
                content: cached.content,
                url: args.uri,
                status: 200,
                content_type: cached.content_type,
                content_length: cached.content_length,
                cached: true,
            };
            let content = Content::json(output)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            return Ok(CallToolResult::success(vec![content]));
        }

        // Fetch the content
        let content_type = "text/html; charset=utf-8".to_string();
        let response = match fetch_content(&url).await {
            Ok((body, content_length)) => {
                // Cache the response
                cache.put(&url, body.clone(), content_type.clone(), content_length);
                body
            }
            Err(e) => {
                warn!("Fetch error for {}: {}", url, e);
                return Ok(CallToolResult::error(vec![
                    Content::text(format!("Fetch failed: {}", e)),
                ]));
            }
        };

        // Convert HTML to markdown
        let markdown = html_to_markdown(&response);

        // Truncate to max_length
        let content: String = markdown.chars().skip(args.start_index).take(args.max_length).collect();

        let output = FetchOutput {
            content,
            url: args.uri,
            status: 200,
            content_type,
            content_length: response.len(),
            cached: false,
        };

        let content = Content::json(output)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }

    #[tool(
        name = "list-search-engines",
        description = "List all available search engines and their configuration status"
    )]
    async fn list_search_engines(
        &self,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry.lock().await;
        let outputs: Vec<EngineInfoOutput> = registry.list().into_iter().map(Into::into).collect();
        let output = EnginesResponseOutput { engines: outputs };
        let content = Content::json(output)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![content]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler — separate impl (matching litterbox pattern)
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for LensServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Lens MCP server for web search and URL fetching".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Safe DNS resolver — filters private/localhost IPs at resolution time
// ---------------------------------------------------------------------------

/// A DNS resolver that intercepts hostname lookups and blocks RFC 1918,
/// loopback, and link-local addresses.
///
/// Implements `reqwest::dns::Resolve` so it can be injected via
/// `ClientBuilder::dns_resolver()`.  Bare IPs are handled separately in
/// `safety::validate_url()` before any network call.
pub struct SafeDnsResolver;

impl Default for SafeDnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl SafeDnsResolver {
    pub fn new() -> Self {
        Self
    }
}

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

impl reqwest::dns::Resolve for SafeDnsResolver {
    fn resolve(
        &self,
        host: reqwest::dns::Name,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Iterator<Item = SocketAddr> + Send>, Box<dyn std::error::Error + Send + Sync>>> + Send>> {
        let host_str = host.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = match tokio::net::lookup_host((host_str.as_str(), 0)).await {
                Ok(iter) => iter.collect(),
                Err(e) => {
                    let err: Box<dyn std::error::Error + Send + Sync> = std::io::Error::other(format!("DNS resolution failed: {e}")).into();
                    return Err(err);
                }
            };

            let safe_addrs: Vec<SocketAddr> = addrs
                .into_iter()
                .filter(|addr| !is_blocked_ip(addr.ip()))
                .collect();

            if safe_addrs.is_empty() {
                let err: Box<dyn std::error::Error + Send + Sync> = std::io::Error::other("DNS resolved to private/localhost IP — blocked for safety").into();
                return Err(err);
            }

            Ok(Box::new(safe_addrs.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>)
        })
    }
}

/// Check if an IP address is in a blocked range.
fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            let first = octets[0];
            let second = octets[1];

            if first == 127 { return true; }
            if first == 10 { return true; }
            if first == 172 && (16..=31).contains(&second) { return true; }
            if first == 192 && second == 168 { return true; }
            if first == 169 && second == 254 { return true; }
            if first == 0 { return true; }
            false
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() { return true; }
            if v6.is_unicast_link_local() { return true; }
            let bytes = v6.octets();
            let first = bytes[0];
            if (first & 0xFE) == 0xFC { return true; }
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_ip(std::net::IpAddr::V4(mapped));
            }
            false
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP fetch + HTML-to-markdown
// ---------------------------------------------------------------------------

#[allow(dead_code)]
async fn fetch_content(uri: &str) -> Result<(String, usize), String> {
    let client = reqwest::Client::builder()
        .dns_resolver(std::sync::Arc::new(SafeDnsResolver::new()))
        .timeout(std::time::Duration::from_secs(30))
        .gzip(true)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let response = client.get(uri).send().await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status().as_u16();
    if status >= 400 {
        let reason = response.status().canonical_reason().unwrap_or("Unknown");
        return Err(format!("HTTP {} {}", status, reason));
    }

    let body = response.text().await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let len = body.len();
    Ok((body, len))
}

fn html_to_markdown(html: &str) -> String {
    let opts = html_to_markdown_rs::ConversionOptions::default();
    match html_to_markdown_rs::convert(html, Some(opts)) {
        Ok(result) => result.content.unwrap_or_default(),
        Err(_) => {
            // Fallback: strip HTML tags and return plain text
            let stripped = regex::Regex::new(r"<[^>]*>")
                .map(|re| re.replace_all(html, "").to_string())
                .unwrap_or_else(|_| html.to_string());
            // Collapse whitespace
            let collapsed = regex::Regex::new(r"\s+")
                .map(|re| re.replace_all(&stripped, " ").to_string())
                .unwrap_or_else(|_| stripped);
            collapsed.trim().to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Start the MCP server, blocking until the connection is closed.
pub async fn run_stdio(cfg: config::Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = LensServer::from_config(&cfg)?;

    let (stdin, stdout) = stdio();
    let running = serve_server(server, (stdin, stdout)).await?;

    // `serve_server` returns a `RunningService` — we must call `.waiting()`
    // to drive the server to completion.  On clean exit `waiting()` returns
    // `Ok(QuitReason)` (Cancelled = stdin EOF, Closed = explicit shutdown);
    // on abnormal exit it returns `Err(JoinError)`.
    let _ = running.waiting().await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// MCP documentation generation data
// ---------------------------------------------------------------------------

use agentkit_docgen::{ParamDoc, ToolDoc};

pub static TOOL_DOCS: &[ToolDoc] = &[
    ToolDoc {
        name: "search",
        description: "Search the web using a configured search engine.",
        params: &[
            ParamDoc {
                name: "query",
                type_name: "string",
                required: true,
                description: "Search query (max 400 chars, 50 words).",
            },
            ParamDoc {
                name: "engine",
                type_name: "string",
                required: true,
                description: "Search engine name (e.g., `brave`).",
            },
            ParamDoc {
                name: "page",
                type_name: "integer",
                required: false,
                description: "Page number (default: 1).",
            },
            ParamDoc {
                name: "max_results",
                type_name: "integer",
                required: false,
                description: "Maximum results per page (default: 10, max 20).",
            },
            ParamDoc {
                name: "region",
                type_name: "string",
                required: false,
                description: "Region/language code (e.g., `en-US`).",
            },
        ],
    },
    ToolDoc {
        name: "fetch",
        description: "Fetch a URL and return its content as markdown.",
        params: &[
            ParamDoc {
                name: "uri",
                type_name: "string",
                required: true,
                description: "URL to fetch (http/https only; LAN IPs blocked).",
            },
            ParamDoc {
                name: "max_length",
                type_name: "integer",
                required: false,
                description: "Maximum output length in characters (default: 8000).",
            },
            ParamDoc {
                name: "start_index",
                type_name: "integer",
                required: false,
                description: "Start index for truncation (default: 0).",
            },
            ParamDoc {
                name: "format",
                type_name: "string",
                required: false,
                description: "Output format (default: `markdown`).",
            },
        ],
    },
    ToolDoc {
        name: "list-search-engines",
        description: "List all available search engines and their configuration status.",
        params: &[],
    },
];
