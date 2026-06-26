#!/usr/bin/env bash
# Mock credential helper for testing
set -e

case "${1:-}" in
    get)
        echo '{"access_token": "mock_access_token", "refresh_token": "mock_refresh", "expires_at": "2026-12-31T23:59:59Z"}'
        exit 0
        ;;
    store)
        cat > /dev/null
        exit 0
        ;;
    erase)
        exit 0
        ;;
    *)
        echo "unknown command: $1" >&2
        exit 2
        ;;
esac
