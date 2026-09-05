use std::process::ExitCode;

fn service_name() -> &'static str {
    "agentkit-credential-keychain"
}

fn init_store() {
    #[cfg(target_os = "linux")]
    {
        let store = dbus_secret_service_keyring_store::Store::new()
            .expect("failed to create secret-service store");
        keyring_core::set_default_store(store);
    }
    #[cfg(target_os = "macos")]
    {
        let store = apple_native_keyring_store::keychain::Store::new()
            .expect("failed to create keychain store");
        keyring_core::set_default_store(store);
    }
    #[cfg(target_os = "windows")]
    {
        let store = windows_native_keyring_store::Store::new()
            .expect("failed to create Windows credential store");
        keyring_core::set_default_store(store);
    }
}

fn entry(component: &str, identity: &str) -> Result<keyring_core::Entry, keyring_core::Error> {
    let account = format!("{component}_{identity}");
    keyring_core::Entry::new(service_name(), &account)
}

fn main() -> ExitCode {
    init_store();

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: agentkit-credential-keychain <get|put|delete|location> [<component>] [<identity>]");
        return ExitCode::from(2);
    }

    let command = &args[1];

    if command == "location" {
        println!("system keychain (service: {})", service_name());
        return ExitCode::SUCCESS;
    }

    if args.len() < 4 {
        eprintln!("usage: agentkit-credential-keychain <get|put|delete|location> <component> <identity>");
        return ExitCode::from(2);
    }

    let component = &args[2];
    let identity = &args[3];

    match command.as_str() {
        "get" => cmd_get(component, identity),
        "put" => cmd_put(component, identity),
        "delete" => cmd_delete(component, identity),
        _ => {
            eprintln!("unknown command: {command}");
            ExitCode::from(2)
        }
    }
}

fn cmd_get(component: &str, identity: &str) -> ExitCode {
    let entry = match entry(component, identity) {
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

fn cmd_put(component: &str, identity: &str) -> ExitCode {
    let blob = match agentkit_credentials::read_stdin() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let json = serde_json::to_string(&blob).unwrap_or_default();
    let entry = match entry(component, identity) {
        Ok(e) => e,
        Err(_) => return ExitCode::FAILURE,
    };
    match entry.set_password(&json) {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

fn cmd_delete(component: &str, identity: &str) -> ExitCode {
    let entry = match entry(component, identity) {
        Ok(e) => e,
        Err(_) => return ExitCode::SUCCESS,
    };
    match entry.delete_credential() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
