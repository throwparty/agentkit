#!/usr/bin/env bash
set -e
PROXY_PORT=3812

cd /home/lukecarrier/Code/throwparty/agentkit

cargo build -p agentkit-switchboard -q 2>/dev/null

RUST_LOG=debug direnv exec . ./target/debug/agentkit-switchboard --config switchboard-e2e.toml > /tmp/switchboard-e2e.log 2>&1 &
PID=$!

for i in $(seq 1 10); do
  if curl -s -o /dev/null http://127.0.0.1:$PROXY_PORT/health 2>/dev/null; then
    break
  fi
  sleep 1
done

RESP=$(curl -s -w "\nHTTP_CODE:%{http_code}\nPROVIDER:%{header{X-Switchboard-Provider}}" \
  http://127.0.0.1:$PROXY_PORT/openai/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5.4-mini","messages":[{"role":"user","content":"hi"}],"stream":false}')

kill $PID 2>/dev/null
wait $PID 2>/dev/null || true

HTTP_CODE=$(echo "$RESP" | grep "HTTP_CODE:" | cut -d: -f2)
PROVIDER=$(echo "$RESP" | grep "PROVIDER:" | cut -d: -f2)
BODY=$(echo "$RESP" | sed -n '/^{/p')

echo "=== RESULT ==="
echo "HTTP: $HTTP_CODE"
echo "Provider: $PROVIDER"
echo "Body: $BODY"
echo "=== PROXY LOG ==="
cat /tmp/switchboard-e2e.log | grep -E '\[forward\]|\[route\]|\[credential' || true

if [ "$HTTP_CODE" = "200" ]; then
  echo "=== SUCCESS ==="
  exit 0
else
  echo "=== FAILED ==="
  exit 1
fi
