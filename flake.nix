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
      ...
    }:
      flake-utils.lib.eachDefaultSystem (system: let
        overlays = [(import rust-overlay)];
        pkgs = import nixpkgs {
          inherit system overlays;
        } // import ./pkgs { inherit pkgs; };

        rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
          extensions = ["rust-src" "clippy"];
          # targets = [];
        };

        nativeBuildInputs = with pkgs; [
          rustToolchain
          pkg-config
          ps5-payload-sdk
        ];
      in {
        packages = import ./pkgs { inherit pkgs; };
        devShells.default = pkgs.mkShell {
          name = "wireguard-client-ps5-shell";

          inherit nativeBuildInputs;

          # Specify the rust-src path (many editors rely on this)
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
        };
      });
}
