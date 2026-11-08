{ stdenv, lib, pkgs, ... }:
let
  project_url = "https://github.com/ps5-payload-dev/sdk";
  pname = "ps5-payload-sdk";
  version = "0.45";

  src = pkgs.fetchurl {
    url = "${project_url}/releases/download/v${version}/ps5-payload-sdk.zip";
    hash = "sha256-X6xkCT0OPmnyWEznk18k9wP2ItloHxSVyFufANEpGiw=";
  };

  nativeBuildInputs = with pkgs; [
    unzip
    autoPatchelfHook
  ];

  destDir = "opt/${pname}";
in
stdenv.mkDerivation {
  inherit
    pname
    version
    src
    nativeBuildInputs;

  sourceRoot = "./${pname}";

  autoPatchelfIgnoreMissingDeps = [ "*.sprx" ];

  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall

    DEST_DIR=$out/${destDir}

    mkdir -p $DEST_DIR
    cp -r bin ldscripts samples target toolchain $DEST_DIR/

    # ln against the binaries.
    mkdir -p $out/bin
    for f in $DEST_DIR/bin/*; do
      ln -s "$f" $out/bin/$(basename "$f")
    done

    # Install environment variables pointing to the SDK.
    mkdir -p $out/nix-support
    echo "export PS5_PAYLOAD_SDK=$DEST_DIR" > $out/nix-support/setup-hook

    runHook postInstall
  '';

  meta = {
    homepage = project_url;
    description = "SDK for developing payload binaries for jailbroken PS5s";
    platforms = lib.platforms.linux;
  };
}
