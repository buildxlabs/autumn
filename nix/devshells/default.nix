{
  pkgs,
  rustToolchain,
  nodeToolchain,
}:
let
  inherit (pkgs.stdenv.hostPlatform) isDarwin;
in
pkgs.mkShell {
  packages = with pkgs; [
    # Rust. The toolchain carries rustc, cargo (the fork), rustfmt, clippy,
    # rust-src and rust-analyzer, and its setup hook exports RUST_SRC_PATH.
    rustToolchain
    openssl
    pkg-config
    bacon
    cargo-deny
    cargo-machete
    cargo-nextest
    taplo

    # Node.js is the runtime pnpm and vp need; there is no separate Node.js
    # toolchain here. The versions come from package.json via crucible.
    nodeToolchain

    # Markdown formatting is Oxfmt via `vp fmt`. typos catches misspellings
    # in prose and comments; it is a Rust binary, so no extra runtime.
    typos

    just
    nixfmt
  ];

  # Leave buildInputs empty on Darwin so Nix does not pull its SDK into the
  # shell; xcrun should use the host's toolchain.
  buildInputs = [ ];

  # OpenSSL crates (openssl-sys, native-tls) find the store build without
  # pkg-config having to guess. `pkgs.openssl` is the bin output, so name the
  # dev (headers, pkg-config) and out (libraries) outputs explicitly.
  OPENSSL_DIR = "${pkgs.openssl.dev}";
  OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
  OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
  PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";

  shellHook = ''
    ${pkgs.lib.optionalString isDarwin ''
      # Keep the linker on the host SDK rather than a Nix-injected one.
      unset DEVELOPER_DIR
      unset SDKROOT
      if [ -x /usr/bin/xcrun ]; then
        # Exporting SDKROOT stops rustc from probing with Nix's xcrun shim,
        # which cannot find modern macOS SDKs.
        SDKROOT="$(/usr/bin/xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
        if [ -n "$SDKROOT" ]; then export SDKROOT; fi
      fi
    ''}

    # Binaries pnpm installed for this project (vp, vitest, ...) come first.
    # The crucible toolchain's setup hook has already put its own node, pnpm
    # and vp on PATH behind this directory.
    export PATH="$PWD/node_modules/.bin:$PATH"

    # Nix's Darwin stdenv adds an old xcbuild `xcrun` shim that cannot find
    # modern SDKs. Drop it so the host /usr/bin/xcrun is used instead.
    export PATH="$(printf '%s' "$PATH" | tr ':' '\n' | grep -v -- '-xcbuild-.*-xcrun' | paste -s -d ':')"

    echo "Autumn dev environment"
    echo "  rust:  $(cargo --version)"
    echo "  pnpm:  $(pnpm --version)"
    echo "  vite+: $(vp --version 2>/dev/null | head -n 1 || echo 'not installed')"
  '';
}
