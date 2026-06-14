# E2E Testing: Switchboard with OpenAI Subscription + API Key

## Setup

Create `~/switchboard-e2e.toml`:

```toml
[[providers]]
identity = "chatgpt_sub"
api_surface = "openai"
base_url = "https://chatgpt.com/backend-api/codex"
billing = "subscription"
models = ["gpt-4o"]

[providers.auth]
type = "openai_codex_oauth"

[providers.auth.oauth]
authorize_url = "https://auth.openai.com/oauth/authorize"
token_url = "https://auth.openai.com/oauth/token"
scopes = "openid profile email offline_access"

[providers.pricing]
input_per_mtok = 0
output_per_mtok = 0

[[providers]]
identity = "openai_payg"
api_surface = "openai"
base_url = "https://api.openai.com/v1"
billing = "pay_as_you_go"
models = ["gpt-4o"]

[providers.auth]
type = "bearer_token"

[providers.pricing]
input_per_mtok = 2.50
output_per_mtok = 10.00
```

Set API key and authenticate subscription:

```bash
export AGENTKIT_SWITCHBOARD_OPENAI_PAYG_API_KEY="sk-..."
cargo run -p agentkit-switchboard -- --config ~/switchboard-e2e.toml auth login chatgpt_sub
```

Start proxy:

```bash
cargo run -p agentkit-switchboard -- --config ~/switchboard-e2e.toml
```

## Scenario A: Subscription routing

```bash
curl -s http://127.0.0.1:3812/openai/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"stream":false}' \
  -w "\nHTTP %{http_code}\nX-Switchboard-Provider: %{header{X-Switchboard-Provider}}\n"
```

Expected: HTTP 200, `X-Switchboard-Provider: chatgpt_sub`.

## Scenario B: Session affinity

```bash
curl -s http://127.0.0.1:3812/openai/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "X-Session-Id: sess-e2e-1" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"stream":false}' \
  -w "\nProvider: %{header{X-Switchboard-Provider}}\n"
```

Run twice. Assert `X-Switchboard-Provider` is the same both times.
