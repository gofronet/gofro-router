{
  description = "Gofro Router — Rust OpenWrt agent/relay/server, Svelte panel and shell helpers";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      # Keep in sync with Cargo.toml and .github/workflows/{ci,release}.yml.
      rustVersion = "1.99.0";
      muslTargets = [
        "aarch64-unknown-linux-musl"
        "armv5te-unknown-linux-musleabi"
        "arm-unknown-linux-musleabihf"
        "armv7-unknown-linux-musleabi"
        "armv7-unknown-linux-musleabihf"
        "i586-unknown-linux-musl"
        "riscv64gc-unknown-linux-musl"
        "x86_64-unknown-linux-musl"
      ];

      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          inherit (pkgs) lib;

          rust = pkgs.rust-bin.stable.${rustVersion}.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "rustfmt"
              "clippy"
              "llvm-tools"
            ];
            targets = muslTargets;
          };

          # Rust workspace, web panel and shell test dependencies.
          common = with pkgs; [
            rust
            bun
            git
            cacert
            curl
            jq
            python3
            shellcheck
            shfmt
            gnumake
            cmake
            pkg-config
            openssl
            perl
            gawk
            gnugrep
            gnused
            findutils
            coreutils
            procps
            which
          ];

          # Root-only helpers used by deploy/openwrt/tests/*.sh and the
          # dataplane/DNS network-namespace checks.
          linuxTools = with pkgs; [
            iproute2
            nftables
            conntrack-tools
            dnsmasq
          ];

          # Static musl cross builds; the release workflow uses zig + cargo-zigbuild.
          crossTools = with pkgs; [
            cargo-zigbuild
            zig
            qemu
          ];

          mkShell =
            extra:
            pkgs.mkShell {
              packages = common ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux (linuxTools ++ extra);

              shellHook = ''
                echo "gofro dev shell — rust $(rustc --version | cut -d' ' -f2), bun $(bun --version)"
                echo "  cargo test --workspace --locked"
                echo "  cargo clippy --workspace --all-targets --locked -- -D warnings"
                echo "  sh deploy/openwrt/tests/guard.sh"
                echo "  (cd web && bun install && bun run check && bun test && bun run build)"
              '';
            };
        in
        {
          default = mkShell [ ];
          cross = mkShell crossTools;
        }
      );

      formatter = forAllSystems (system: (import nixpkgs { inherit system; }).nixfmt);
    };
}
