fn mutate(request) {
    request.body["tools"] = [#{
        "type": "function",
        "function": #{
            "name": "bash",
            "description": "Run a shell command",
            "parameters": #{
                "type": "object",
                "properties": #{ "command": #{ "type": "string" } },
                "required": ["command"]
            }
        }
    }];
    request.body["tool_choice"] = "none";
    request
}
