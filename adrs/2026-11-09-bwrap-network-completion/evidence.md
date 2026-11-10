# Host network proof

Date: 2026-11-10
Kernel: 7.2.15
Host: luke-c0nstruct

## Commands

```text
cargo test -p agentkit-litterbox a_real_host_sandbox_shares_the_network -- --ignored --nocapture --test-threads=1
cargo test -p agentkit-litterbox a_real_denied_sandbox_isolates_the_network -- --ignored --nocapture --test-threads=1
```

## Host

```text
host net namespace: "net:[4026531833]"
sandbox net namespace: "net:[4026531833]"
command net namespace: net:[4026531833]
listener received: hello from the host
inspection: egress Direct forwards [Forward { name: "web", target: 46759, host_port: 46759, bind: "127.0.0.1", env_var: "LITTERBOX_FWD_PORT_WEB" }]
```

The other six namespaces differ:

```text
user namespace: host "user:[4026531837]" sandbox "user:[4026534363]"
ipc namespace: host "ipc:[4026531839]" sandbox "ipc:[4026534360]"
pid namespace: host "pid:[4026531836]" sandbox "pid:[4026534361]"
mnt namespace: host "mnt:[4026531832]" sandbox "mnt:[4026534358]"
uts namespace: host "uts:[4026531838]" sandbox "uts:[4026534359]"
cgroup namespace: host "cgroup:[4026531835]" sandbox "cgroup:[4026534362]"
```

## Denied

```text
host net namespace: "net:[4026531833]"
sandbox net namespace: "net:[4026534363]"
host connection: Some(Os { code: 111, kind: ConnectionRefused, message: "Connection refused" })
inspection: egress Denied forwards []
```

## Host network namespace cannot be entered

On a live host sandbox (pid 773841):

```text
$ nsenter --net=/proc/773841/ns/net true
nsenter: reassociate to namespace 'ns/net' failed: Operation not permitted
exit=1
```

This is why the code skips the network namespace under `host`: the namespace is
owned by the initial user namespace and this process has no capability there.
The join still checks the recorded network number, and the command's own
`/proc/self/ns/net` is the host's number.
