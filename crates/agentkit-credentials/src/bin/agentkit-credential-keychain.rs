use std::process::ExitCode;

fn service_name() -> &'static str {
    "agentkit-credential-keychain"
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
    keyring::Entry::new(service_name(), identity)
}

fn cmd_get(identity: &str) -> ExitCode {
    let entry = match entry(identity) {
        Ok(e) => e,
        Err(_) => return ExitCode::from(1),
    };
    let password = match entry.get_password() {
        Ok(p) => p,
        Err(_) => return ExitCode::from(1),
    };
    println!("{password}");
    ExitCode::SUCCESS
}

fn cmd_store(identity: &str) -> ExitCode {
    let blob = match agentkit_credentials::read_stdin() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let json = serde_json::to_string(&blob).unwrap_or_default();
    let entry = match entry(identity) {
        Ok(e) => e,
        Err(_) => return ExitCode::FAILURE,
    };
    match entry.set_password(&json) {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn cmd_erase(identity: &str) -> ExitCode {
    let entry = match entry(identity) {
        Ok(e) => e,
        Err(_) => return ExitCode::SUCCESS,
    };
    match entry.delete_credential() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
