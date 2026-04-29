import json
import os
import subprocess
import sys

env = os.environ.copy()
p = subprocess.Popen(
    ["target/debug/litterbox", "stdio"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
    env=env,
)


def send(obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()


def recv():
    line = p.stdout.readline()
    if not line:
        sys.exit("server closed stdout")
    print(line.strip())
    return line


send(
    {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "cli", "version": "0.0.0"},
            "capabilities": {},
        },
    }
)
recv()

send({"jsonrpc": "2.0", "method": "notifications/initialized"})
send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
recv()

send(
    {
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "sandbox-create", "arguments": {"name": "woo"}},
    }
)
recv()
