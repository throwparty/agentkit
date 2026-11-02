let
  pkgs = import (builtins.getFlake "nixpkgs") { system = builtins.currentSystem; };
  euid = pkgs.writeText "euid.c" ''
    #include <stdio.h>
    #include <unistd.h>
    int main(void) { printf("%d\n", geteuid()); return 0; }
  '';
in
pkgs.testers.nixosTest {
  name = "litterbox-bwrap-policy";

  nodes.enabled =
    { pkgs, ... }:
    {
      environment.systemPackages = [
        pkgs.bubblewrap
        pkgs.gcc
        pkgs.python3
        pkgs.util-linux
      ];
      users.users.tester = {
        isNormalUser = true;
        shell = pkgs.bash;
      };
      virtualisation.memorySize = 2048;
    };

  nodes.disabled =
    { pkgs, ... }:
    {
      boot.kernel.sysctl."user.max_user_namespaces" = 0;
      environment.systemPackages = [
        pkgs.bubblewrap
        pkgs.util-linux
      ];
      virtualisation.memorySize = 2048;
    };

  testScript = ''
    start_all()
    disabled.wait_for_unit("multi-user.target")
    enabled.wait_for_unit("multi-user.target")

    disabled.fail("unshare -U true")
    disabled.fail("bwrap --unshare-user -- true")

    enabled.succeed("unshare -U true")
    enabled.succeed("bwrap --unshare-user --ro-bind /nix/store /nix/store --ro-bind /run/current-system /run/current-system -- /run/current-system/sw/bin/true")

    enabled.succeed("gcc ${euid} -o /tmp/euid")
    enabled.succeed("chown root:root /tmp/euid && chmod 4755 /tmp/euid")
    enabled.succeed("su tester -c '/tmp/euid' | grep -qx 0")
    enabled.succeed("su tester -c 'bwrap --ro-bind /nix/store /nix/store --ro-bind /tmp/euid /euid -- /euid' | grep -vqx 0")

    enabled.succeed("bwrap --unshare-user --proc /proc --ro-bind /nix/store /nix/store --ro-bind /run/current-system /run/current-system -- /run/current-system/sw/bin/true")
  '';
}
