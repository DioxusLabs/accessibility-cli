{
  description = "Nix flake for accessibility-cli development";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = f:
        nixpkgs.lib.genAttrs systems (system:
          f {
            pkgs = import nixpkgs {
              inherit system;
            };
          });
    in
    {
      packages = forEachSystem ({ pkgs }: {
        run-linux-e2e-tests = pkgs.writeShellApplication {
          name = "run-linux-e2e-tests";
          runtimeInputs = with pkgs; [
            cargo
            rustc
            dbus
            gnome-calculator
            wmctrl
            xdotool
            xauth
            xorg.xorgserver
          ];
          text = ''
            export GTK_MODULES="gail:atk-bridge"
            display_num=99
            while [ -e "/tmp/.X''${display_num}-lock" ]; do
              display_num=$((display_num + 1))
            done

            export DISPLAY=":''${display_num}"
            Xvfb "$DISPLAY" -screen 0 1440x900x24 >/tmp/accessibility-cli-xvfb.log 2>&1 &
            xvfb_pid=$!

            cleanup() {
              kill "$xvfb_pid" >/dev/null 2>&1 || true
              wait "$xvfb_pid" >/dev/null 2>&1 || true
            }
            trap cleanup EXIT INT TERM

            sleep 1

            dbus-run-session -- cargo test -p accessibility-core --test gnome_calculator_e2e -- --nocapture "$@"
          '';
        };
      });

      devShells = forEachSystem ({ pkgs }: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            self.packages.${pkgs.stdenv.hostPlatform.system}.run-linux-e2e-tests
            dbus
            gnome-calculator
            wmctrl
            xdotool
            xauth
            xorg.xorgserver
            dbus.dev
            at-spi2-core.dev
            libx11.dev
            libxcb.dev
          ];

          shellHook = ''
            echo "Loaded accessibility-cli development shell"
            echo "Rust toolchain: $(rustc --version)"
            ${pkgs.lib.optionalString pkgs.stdenv.isLinux ''
              cleanup_accessibility_cli_shell() {
                if [ -n "''${ACCESSIBILITY_CLI_XVFB_PID:-}" ]; then
                  kill "''${ACCESSIBILITY_CLI_XVFB_PID}" >/dev/null 2>&1 || true
                  wait "''${ACCESSIBILITY_CLI_XVFB_PID}" >/dev/null 2>&1 || true
                fi
                if [ -n "''${ACCESSIBILITY_CLI_DBUS_PID:-}" ]; then
                  kill "''${ACCESSIBILITY_CLI_DBUS_PID}" >/dev/null 2>&1 || true
                  wait "''${ACCESSIBILITY_CLI_DBUS_PID}" >/dev/null 2>&1 || true
                fi
              }
              trap cleanup_accessibility_cli_shell EXIT

              if [ -z "''${DISPLAY:-}" ]; then
                display_num=99
                while [ -e "/tmp/.X''${display_num}-lock" ]; do
                  display_num=$((display_num + 1))
                done
                export DISPLAY=":''${display_num}"
                Xvfb "$DISPLAY" -screen 0 1440x900x24 >/tmp/accessibility-cli-xvfb.log 2>&1 &
                export ACCESSIBILITY_CLI_XVFB_PID=$!
                sleep 1
                echo "Started Xvfb on $DISPLAY"
              fi

              dbus_info="$(${pkgs.dbus}/bin/dbus-daemon --session --fork --print-address=1 --print-pid=1)"
              export DBUS_SESSION_BUS_ADDRESS="$(printf '%s\n' "$dbus_info" | sed -n '1p')"
              export ACCESSIBILITY_CLI_DBUS_PID="$(printf '%s\n' "$dbus_info" | sed -n '2p')"
              echo "Started D-Bus session: $DBUS_SESSION_BUS_ADDRESS"

              echo "Linux GUI E2E runner: run-linux-e2e-tests"
            ''}
          '';
        };
      });
    };
}
