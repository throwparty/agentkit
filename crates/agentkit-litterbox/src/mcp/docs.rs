#[derive(Clone, Copy)]
pub(super) struct ToolDoc {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) params: &'static [ParamDoc],
}

#[derive(Clone, Copy)]
pub(super) struct ParamDoc {
    pub(super) name: &'static str,
    pub(super) type_name: &'static str,
    pub(super) required: bool,
    pub(super) description: &'static str,
}

pub(super) const SANDBOX_NAME_PARAM: ParamDoc = ParamDoc {
    name: "sandbox",
    type_name: "string",
    required: true,
    description: "Sandbox name.",
};

pub(super) const PATH_PARAM: ParamDoc = ParamDoc {
    name: "path",
    type_name: "string",
    required: true,
    description: "Path inside the sandbox.",
};

pub(super) const TOOL_DOCS: &[ToolDoc] = &[
    ToolDoc {
        name: "sandbox-create",
        description: "Create a new sandbox based on the current repository HEAD.",
        params: &[ParamDoc {
            name: "name",
            type_name: "string",
            required: true,
            description: "Sandbox name.",
        }],
    },
    ToolDoc {
        name: "sandbox-ports",
        description: "Get forwarded ports for a sandbox.",
        params: &[SANDBOX_NAME_PARAM],
    },
    ToolDoc {
        name: "read",
        description: "Read a file from the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "offset",
                type_name: "integer",
                required: false,
                description: "Line offset (0-based).",
            },
            ParamDoc {
                name: "limit",
                type_name: "integer",
                required: false,
                description: "Maximum number of lines to read.",
            },
        ],
    },
    ToolDoc {
        name: "write",
        description: "Write a file into the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "content",
                type_name: "string",
                required: true,
                description: "Full file contents to write.",
            },
        ],
    },
    ToolDoc {
        name: "patch",
        description: "Apply a unified diff inside the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "diff",
                type_name: "string",
                required: true,
                description: "Unified diff to apply.",
            },
        ],
    },
    ToolDoc {
        name: "bash",
        description: "Execute a shell command inside the sandbox.",
        params: &[
            SANDBOX_NAME_PARAM,
            ParamDoc {
                name: "command",
                type_name: "string",
                required: true,
                description: "Shell command to execute.",
            },
            ParamDoc {
                name: "workdir",
                type_name: "string",
                required: false,
                description: "Working directory inside the sandbox.",
            },
            ParamDoc {
                name: "timeout",
                type_name: "integer",
                required: false,
                description: "Timeout in seconds.",
            },
        ],
    },
    ToolDoc {
        name: "ls",
        description: "List directory entries.",
        params: &[
            SANDBOX_NAME_PARAM,
            PATH_PARAM,
            ParamDoc {
                name: "recursive",
                type_name: "boolean",
                required: false,
                description: "Recursively list all descendants.",
            },
        ],
    },
    ToolDoc {
        name: "glob",
        description: "Find files matching a glob pattern.",
        params: &[
            SANDBOX_NAME_PARAM,
            ParamDoc {
                name: "pattern",
                type_name: "string",
                required: true,
                description: "Glob pattern to match.",
            },
            ParamDoc {
                name: "path",
                type_name: "string",
                required: false,
                description: "Base path for matching.",
            },
        ],
    },
    ToolDoc {
        name: "grep",
        description: "Search file contents for a pattern.",
        params: &[
            SANDBOX_NAME_PARAM,
            ParamDoc {
                name: "pattern",
                type_name: "string",
                required: true,
                description: "Pattern to search for.",
            },
            PATH_PARAM,
            ParamDoc {
                name: "include",
                type_name: "string",
                required: false,
                description: "Glob include filter (e.g., *.rs).",
            },
        ],
    },
];
