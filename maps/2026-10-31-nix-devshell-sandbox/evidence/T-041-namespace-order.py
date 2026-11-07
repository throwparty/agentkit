#!/usr/bin/env python3
"""Probe for T-041: which user namespace owns the sandbox namespaces, and which
join order succeeds.

    python3 maps/2026-10-31-nix-devshell-sandbox/evidence/T-041-namespace-order.py

Starts a holder, prints the owner of every namespace with NS_GET_USERNS, tries
the order that nsenter accepts (sandbox user namespace first), then tries the
order that works (the owning user namespace first). Kills the holder on exit.
"""

import contextlib
import ctypes
import os
import subprocess
import sys
import tempfile
import time

CLONE = {
    "user": 0x10000000,
    "ipc": 0x08000000,
    "net": 0x40000000,
    "pid": 0x20000000,
    "mnt": 0x00020000,
    "uts": 0x04000000,
    "cgroup": 0x02000000,
}
NS_GET_USERNS = 0xB701
BWRAP = os.environ.get(
    "BWRAP", "/nix/store/mvj6y8k3xs35cj7kq937xi44q4hl3r93-bubblewrap-0.12.0/bin/bwrap"
)

state = {"holder": None, "work": None}


def cleanup():
    holder = state["holder"]
    if holder is not None and holder.poll() is None:
        holder.kill()
        holder.wait()
    if state["work"] is not None:
        subprocess.run(["rm", "-rf", state["work"]], check=False)


def start_holder(root, status_path):
    argv = [
        BWRAP,
        "--unshare-user", "--unshare-pid", "--unshare-ipc", "--unshare-uts",
        "--unshare-cgroup", "--unshare-net", "--disable-userns",
        "--clearenv", "--setenv", "PATH", "/run/current-system/sw/bin",
        "--ro-bind", "/nix/store", "/nix/store",
        "--ro-bind", "/run/current-system", "/run/current-system",
        "--ro-bind", "/etc", "/etc",
        "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
        "--dir", "/src", "--bind", root, "/src", "--chdir", "/src",
        "--json-status-fd", "3",
        "--", "sleep", "2147483647",
    ]
    fd = os.open(status_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    if fd != 3:
        os.dup2(fd, 3)
        os.close(fd)
        fd = 3
    process = subprocess.Popen(
        argv, pass_fds=(fd,), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL
    )
    os.close(fd)
    return process


def read_status(path, timeout=15.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(path) as status_file:
                text = status_file.read()
        except FileNotFoundError:
            text = ""
        if "child-pid" in text:
            for part in text.replace("{", "").replace("}", "").split(","):
                name, _, value = part.partition(":")
                if name.strip().strip('"') == "child-pid":
                    return int(value.strip())
        time.sleep(0.01)
    raise RuntimeError("no status document")


def owner_of(pid, name):
    fd = os.open(f"/proc/{pid}/ns/{name}", os.O_RDONLY)
    owner = libc.ioctl(fd, NS_GET_USERNS, 0)
    if owner < 0:
        error = ctypes.get_errno()
        os.close(fd)
        return f"unavailable: {os.strerror(error)}"
    link = os.readlink(f"/proc/self/fd/{owner}")
    os.close(owner)
    os.close(fd)
    return link


def close_quietly(*descriptors):
    for descriptor in descriptors:
        if descriptor >= 0:
            try:
                os.close(descriptor)
            except OSError:
                # Best-effort cleanup: the descriptor may already be closed.
                pass


def attempt(label, pid, order, use_base):
    names = set(order) | {"user"}
    descriptors = {}
    with contextlib.ExitStack() as stack:
        for name in names:
            descriptor = os.open(f"/proc/{pid}/ns/{name}", os.O_RDONLY)
            stack.callback(os.close, descriptor)
            descriptors[name] = descriptor
        base = libc.ioctl(descriptors["user"], NS_GET_USERNS, 0) if use_base else -1
        if base >= 0:
            stack.callback(os.close, base)
        child = os.fork()
        if child == 0:

            def exit_child(code):
                close_quietly(base, *descriptors.values())
                os._exit(code)

            sequence = []
            if use_base:
                sequence.append(("owning user namespace", base, CLONE["user"]))
            sequence += [(name, descriptors[name], CLONE[name]) for name in order]
            for name, fd, nstype in sequence:
                result = libc.setns(fd, nstype)
                if result != 0:
                    error = ctypes.get_errno()
                    os.write(1, f"{label}: {name} failed: {os.strerror(error)}\n".encode())
                    exit_child(1)
            os.write(1, f"{label}: all joined\n".encode())
            exit_child(0)
        os.waitpid(child, 0)


libc = ctypes.CDLL("libc.so.6", use_errno=True)


def main():
    state["work"] = tempfile.mkdtemp(prefix="nsorder.")
    work = state["work"]
    root = os.path.join(work, "root")
    os.makedirs(root)
    status = os.path.join(work, "status")

    state["holder"] = start_holder(root, status)
    pid = read_status(status)
    time.sleep(0.3)

    print(f"holder userns: {os.readlink(f'/proc/{pid}/ns/user')}")
    for name in CLONE:
        print(f"{name} owner userns: {owner_of(pid, name)}")

    attempt(
        "sandbox user namespace first",
        pid,
        ["user", "ipc", "net", "pid", "mnt", "uts", "cgroup"],
        use_base=False,
    )
    attempt(
        "owning user namespace first",
        pid,
        ["ipc", "net", "pid", "mnt", "uts", "cgroup", "user"],
        use_base=True,
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        cleanup()
