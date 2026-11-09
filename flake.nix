{
  description = "Wireguard client for PS5 dev flake";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    ps5-rust = {
      url = "github:neokuze/ps5-rust";
      flake = false;
    };
  };

  outputs = {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ps5-rust,
      ...
    }:
      flake-utils.lib.eachDefaultSystem (system: let
        overlays = [(import rust-overlay)];
        pkgs = import nixpkgs {
          inherit system overlays;
        } // import ./pkgs { inherit pkgs; };

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile "${ps5-rust}/rust-toolchain.toml";

        stdTarget = "${ps5-rust}/targets/x86_64-sie-ps5-std.json";
        noStdTarget = "${ps5-rust}/targets/x86_64-sie-ps5.json";

        # Build a `.cargo/config.toml` file based on the `ps5-rust`-provided one,
        # but override the `target` and `linker` paths to point to the Nix-specific locations.
        cargoConfig = (pkgs.formats.toml {}).generate "cargo-ps5-config.toml" (
          fromTOML (builtins.readFile "${ps5-rust}/.cargo/config.toml")
          // {
            build.target = noStdTarget;

            target.x86_64-sie-ps5.linker = "${pkgs.ps5-payload-sdk}/bin/prospero-clang";
          }
        );

        # Wrap `cargo` to always supply the above generated config.toml file.
        wrappedCargo = (pkgs.writeShellScriptBin "cargo-ps5" ''
          exec ${rustToolchain}/bin/cargo --config "${cargoConfig}" "$@"
        '');

        nativeBuildInputs = with pkgs; [
          pkg-config
          ps5-payload-sdk
          rustToolchain
          wrappedCargo
        ];
      in {
        packages = import ./pkgs { inherit pkgs; };
        devShells.default = pkgs.mkShell {
          name = "wireguard-client-ps5-shell";

          inherit nativeBuildInputs;

          CARGO_BUILD_TARGET = noStdTarget;
          # Specify the rust-src path (many editors rely on this)
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          CARGO_TARGET_CONFIG = "${cargoConfig}";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };
      });
}
