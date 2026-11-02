use std::env;
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;

fn main() {
    if env::args().skip(1).any(|arg| arg == "--hostile") {
        hostile();
    } else {
        println!("fixture: {}", fixture::value());
    }
}

fn hostile() {
    let host_home = env::var("HOST_HOME").expect("HOST_HOME must be set");
    let mut leaks = 0;

    leaks += report(
        "write-outside",
        fs::write(format!("{host_home}/lbx-write-probe"), b"x").map(|_| ()),
    );
    leaks += report(
        "write-store",
        fs::write("/nix/store/lbx-write-probe", b"x").map(|_| ()),
    );
    leaks += report(
        "read-ssh-key",
        fs::read(format!("{host_home}/.ssh/id_ed25519")).map(|_| ()),
    );
    leaks += report(
        "connect",
        TcpStream::connect("1.1.1.1:443").map(|_| ()),
    );

    match TcpListener::bind("127.0.0.1:0").and_then(|listener| {
        let address = listener.local_addr()?;
        TcpStream::connect(address).map(|_| ())
    }) {
        Ok(()) => println!("loopback: ok"),
        Err(error) => println!("loopback: denied ({error})"),
    }

    if let Ok(host_pid) = env::var("HOST_PID") {
        match Command::new("/bin/sh")
            .arg("-c")
            .arg("kill -0 \"$1\"")
            .arg("sh")
            .arg(&host_pid)
            .output()
        {
            Ok(output) if output.status.success() => {
                println!("signal-host: leak");
                leaks += 1;
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("signal-host: denied ({}: {})", output.status, stderr.trim());
            }
            Err(error) => {
                println!("signal-host: not-run ({error})");
                leaks += 1;
            }
        }
    }

    let child_probe = format!("{host_home}/lbx-child-probe");
    match Command::new("/bin/sh")
        .arg("-c")
        .arg("touch \"$1\"")
        .arg("sh")
        .arg(&child_probe)
        .output()
    {
        Ok(output) if output.status.success() && Path::new(&child_probe).exists() => {
            println!("child-write: leak");
            leaks += 1;
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("child-write: denied ({}: {})", output.status, stderr.trim());
        }
        Err(error) => {
            println!("child-write: not-run ({error})");
            leaks += 1;
        }
    }

    if leaks > 0 {
        eprintln!("{leaks} check(s) leaked");
        std::process::exit(1);
    }
}

fn report(name: &str, result: io::Result<()>) -> i32 {
    match result {
        Ok(()) => {
            println!("{name}: leak");
            1
        }
        Err(error) => {
            println!("{name}: denied ({error})");
            0
        }
    }
}
