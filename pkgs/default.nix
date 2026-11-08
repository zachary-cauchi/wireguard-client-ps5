{ pkgs, ... }: {
  ps5-payload-sdk = pkgs.callPackage ./ps5-payload-sdk.nix { };
}
