{ stdenv, lib, pkgs, ... }:
let
  project_url = "https://github.com/ps5-payload-dev/sdk";
  pname = "ps5-payload-sdk";
  version = "0.45";

  src = pkgs.fetchurl {
    url = "${project_url}/releases/download/v${version}/ps5-payload-sdk.zip";
    hash = "sha256-X6xkCT0OPmnyWEznk18k9wP2ItloHxSVyFufANEpGiw=";
  };

  llvmRev = pkgs.llvmPackages;
  llvmDeps = pkgs.symlinkJoin {
    name = "llvm-fhs-like";
    paths = [
      llvmRev.clang-unwrapped
      llvmRev.lld
      llvmRev.llvm
      llvmRev.bintools-unwrapped
    ];
  };
  llvmConfigShim = pkgs.writeShellScript "prospero-llvm-config" ''
    if [ "$1" = "--bindir" ]; then
    echo ${llvmDeps}/bin
    else
    exec ${lib.getDev llvmRev.llvm}/bin/llvm-config "$@"
    fi
  '';

  nativeBuildInputs = with pkgs; [
    unzip
    autoPatchelfHook
  ];
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

    mkdir -p $out
    cp -r bin ldscripts samples target toolchain $out

    # Replace the wrapper script with a
    ln -sf ${llvmConfigShim} $out/bin/prospero-llvm-config

    # ln against the binaries.
    # mkdir -p $out/bin
    # for f in $DEST_DIR/bin/*; do
    #   ln -s "$f" $out/bin/$(basename "$f")
    # done

    patchShebangs $out/bin

    # Install environment variables pointing to the SDK.
    mkdir -p $out/nix-support
    echo "export PS5_PAYLOAD_SDK=$out" > $out/nix-support/setup-hook

    runHook postInstall
  '';

  meta = {
    homepage = project_url;
    description = "SDK for developing payload binaries for jailbroken PS5s";
    platforms = lib.platforms.linux;
  };
}
