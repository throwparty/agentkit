use std::process::ExitCode;

fn service_name() -> String {
    std::env::var("AGENTKIT_CREDENTIAL_SERVICE")
        .unwrap_or_else(|_| "agentkit-credential-keychain".into())
}

fn diag(msg: &str) {
    eprintln!("[credential-keychain] {msg}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: agentkit-credential-keychain <get|store|erase> <identity>");
        return ExitCode::from(2);
    }

    let command = &args[1];
    let identity = &args[2];

    match command.as_str() {
        "get" => cmd_get(identity),
        "store" => cmd_store(identity),
        "erase" => cmd_erase(identity),
        _ => {
            eprintln!("unknown command: {command}");
            ExitCode::from(2)
        }
    }
}

fn entry(identity: &str) -> Result<keyring::Entry, keyring::Error> {
    let svc = service_name();
    diag(&format!("entry: service='{svc}', account='{identity}'"));
    keyring::Entry::new(&svc, identity)
}

fn cmd_get(identity: &str) -> ExitCode {
    let svc = service_name();
    diag(&format!("get: service='{svc}', account='{identity}'"));
    let e = match entry(identity) {
        Ok(e) => {
            diag("entry created");
            e
        }
        Err(e) => {
            diag(&format!("entry failed: {e}"));
            return ExitCode::from(1);
        }
    };
    match e.get_password() {
        Ok(p) => {
            println!("{p}");
            diag("get: success");
            ExitCode::SUCCESS
        }
        Err(e) => {
            diag(&format!("get_password failed: {e}"));
            ExitCode::from(1)
        }
    }
}

fn cmd_store(identity: &str) -> ExitCode {
    let svc = service_name();
    diag(&format!("store: service='{svc}', account='{identity}'"));

    let blob = match agentkit_credentials::read_stdin() {
        Ok(b) => {
            diag(&format!(
                "stdin: {} bytes",
                serde_json::to_string(&b).unwrap_or_default().len()
            ));
            b
        }
        Err(e) => {
            diag(&format!("stdin failed: {e}"));
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let json = serde_json::to_string(&blob).unwrap_or_default();

    let e = match entry(identity) {
        Ok(e) => {
            diag("entry created");
            e
        }
        Err(e) => {
            diag(&format!("entry failed: {e}"));
            return ExitCode::FAILURE;
        }
    };

    diag("calling set_password...");
    match e.set_password(&json) {
        Ok(_) => {
            diag(&format!("set_password: success ({} bytes)", json.len()));
        }
        Err(e) => {
            diag(&format!("set_password failed: {e}"));
            return ExitCode::FAILURE;
        }
    }

    diag("verifying with get_password...");
    match e.get_password() {
        Ok(v) => {
            if v == json {
                diag("verify: match");
            } else {
                diag(&format!(
                    "verify: MISMATCH (got {} bytes, expected {})",
                    v.len(),
                    json.len()
                ));
            }
        }
        Err(e) => {
            diag(&format!("verify failed: {e}"));
        }
    }

    diag("store: success");
    ExitCode::SUCCESS
}

fn cmd_erase(identity: &str) -> ExitCode {
    let svc = service_name();
    diag(&format!("erase: service='{svc}', account='{identity}'"));
    let e = match entry(identity) {
        Ok(e) => e,
        Err(_) => {
            diag("no entry, nothing to erase");
            return ExitCode::SUCCESS;
        }
    };
    match e.delete_credential() {
        Ok(_) => {
            diag("erase: success");
            ExitCode::SUCCESS
        }
        Err(e) => {
            diag(&format!("erase failed: {e}"));
            ExitCode::FAILURE
        }
    }
}
