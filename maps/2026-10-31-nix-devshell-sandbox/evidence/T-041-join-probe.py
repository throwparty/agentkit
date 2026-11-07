#!/usr/bin/env python3
"""Join a live bwrap sandbox in this process, as the backend would.

The order matters. The sandbox's namespaces are owned by the user namespace
that bwrap created as the sandbox's parent, not by the sandbox's own user
namespace. The join enters that parent first, then the namespaces it owns, then
the sandbox's user namespace. Every descriptor is opened before the first join,
because joining the mount namespace replaces /proc.

Spawns its own holder, so it needs bubblewrap. Every exit path kills the holder.
"""

import ctypes
import os
import signal
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
ORDER = ("ipc", "net", "pid", "mnt", "uts", "cgroup")

libc = ctypes.CDLL("libc.so.6", use_errno=True)
BWRAP = os.environ.get(
    "BWRAP", "/nix/store/mvj6y8k3xs35cj7kq937xi44q4hl3r93-bubblewrap-0.12.0/bin/bwrap"
)

holder = None
work = None


def cleanup():
    if holder is not None and holder.poll() is None:
        holder.kill()
        holder.wait()
    if work is not None:
        subprocess.run(["rm", "-rf", work], check=False)


def start_holder(root, status_path, stderr_path):
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
    stderr = open(stderr_path, "wb")
    process = subprocess.Popen(
        argv, pass_fds=(fd,), stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=stderr,
    )
    os.close(fd)
    stderr.close()
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


def mount_table(pid):
    table = {}
    with open(f"/proc/{pid}/mountinfo") as handle:
        for line in handle:
            left, _, _ = line.partition(" - ")
            fields = left.split()
            if len(fields) >= 6:
                table[fields[4]] = fields[5]
    return table


def wait_for_view(pid, expected, timeout=30.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        table = mount_table(pid)
        if all(path in table and access in table[path] for path, access in expected.items()):
            return True
        time.sleep(0.01)
    return False


def close_quietly(*descriptors):
    for descriptor in descriptors:
        if descriptor >= 0:
            try:
                os.close(descriptor)
            except OSError:
                # Best-effort cleanup: the descriptor may already be closed.
                pass


def join(sandbox_pid, command):
    descriptors = {}
    base = -1
    root_fd = -1
    try:
        for name in CLONE:
            descriptors[name] = os.open(
                f"/proc/{sandbox_pid}/ns/{name}", os.O_RDONLY | os.O_CLOEXEC
            )
        base = libc.ioctl(descriptors["user"], NS_GET_USERNS, 0)
        root_fd = os.open(
            f"/proc/{sandbox_pid}/root", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC
        )

        child = os.fork()
        if child == 0:

            def exit_child(code):
                close_quietly(root_fd, base, *descriptors.values())
                os._exit(code)

            try:
                sequence = [("base user", base, CLONE["user"])]
                sequence += [(name, descriptors[name], CLONE[name]) for name in ORDER]
                sequence += [("sandbox user", descriptors["user"], CLONE["user"])]
                for name, fd, nstype in sequence:
                    if libc.setns(fd, nstype) != 0:
                        error = ctypes.get_errno()
                        os.write(2, f"join: setns {name} failed: {os.strerror(error)}\n".encode())
                        exit_child(210)
                close_quietly(base, *descriptors.values())
                grandchild = os.fork()
                if grandchild == 0:
                    os.fchdir(root_fd)
                    os.chroot(".")
                    os.chdir("/src")
                    os.execv(command[0], command)
                    exit_child(211)
                _, status = os.waitpid(grandchild, 0)
                exit_child(os.waitstatus_to_exitcode(status))
            except Exception as error:  # noqa: BLE001
                os.write(2, f"join: {error}\n".encode())
                exit_child(213)
        _, status = os.waitpid(child, 0)
    finally:
        close_quietly(base, root_fd, *descriptors.values())
    return os.waitstatus_to_exitcode(status)


def main():
    global holder, work
    work = tempfile.mkdtemp(prefix="join-probe.")
    root = os.path.join(work, "root")
    os.makedirs(root)
    status = os.path.join(work, "status")
    stderr = os.path.join(work, "bwrap-stderr")

    holder = start_holder(root, status, stderr)
    sandbox = read_status(status)
    print(f"holder: {holder.pid}  sandbox: {sandbox}")

    if not wait_for_view(sandbox, {"/src": "rw", "/nix/store": "ro"}):
        print("view never became ready")
        return 1
    print("view ready")

    code = join(
        sandbox,
        [
            "/run/current-system/sw/bin/sh",
            "-c",
            "readlink /proc/self/ns/pid > /src/joined-pid-ns; "
            + "readlink /proc/self/ns/cgroup > /src/joined-cgroup-ns; "
            + "id -u > /src/joined-uid; echo ok > /src/joined-ok",
        ],
    )
    print(f"joined command exit: {code}")

    with open(os.path.join(root, "joined-pid-ns")) as handle:
        joined = handle.read().strip()
    sandbox_ns = os.readlink(f"/proc/{sandbox}/ns/pid")
    host_ns = os.readlink("/proc/self/ns/pid")
    with open(os.path.join(root, "joined-cgroup-ns")) as handle:
        cgroup = handle.read().strip()
    sandbox_cgroup = os.readlink(f"/proc/{sandbox}/ns/cgroup")
    with open(os.path.join(root, "joined-uid")) as handle:
        uid = handle.read().strip()
    print(f"joined pid namespace:   {joined}")
    print(f"sandbox pid namespace:  {sandbox_ns}")
    print(f"host pid namespace:     {host_ns}")
    print(f"joined is a member:     {'yes' if joined == sandbox_ns else 'no'}")
    print(f"joined cgroup namespace: {'same as sandbox' if cgroup == sandbox_cgroup else 'different'}")
    print(f"joined uid:             {uid}")
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        sys.exit(main())
    finally:
        cleanup()
