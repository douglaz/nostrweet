{
  description = "Rust project with musl target";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        # Minimal toolchain for building (no docs, analyzer, or src)
        rustToolchainMinimal = pkgs.rust-bin.stable.latest.minimal.override {
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        # Build the nostrweet binary
        nostrweet = let
          rustPlatformMusl = pkgs.makeRustPlatform {
            cargo = rustToolchainMinimal;
            rustc = rustToolchainMinimal;
          };
        in rustPlatformMusl.buildRustPackage {
          pname = "nostrweet";
          version = "0.1.0";
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
            rustToolchainMinimal
            pkgsStatic.stdenv.cc
          ];

          buildInputs = with pkgs; [
            pkgsStatic.openssl
          ];

          # Musl target configuration
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}cc";
          CC_x86_64_unknown_linux_musl = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}cc";
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static -C link-arg=-static";

          # Set OpenSSL environment variables for static linking
          OPENSSL_STATIC = "1";
          OPENSSL_LIB_DIR = "${pkgs.pkgsStatic.openssl}/lib";
          OPENSSL_INCLUDE_DIR = "${pkgs.pkgsStatic.openssl.dev}/include";

          # Override buildPhase to use the correct target
          buildPhase = ''
            runHook preBuild

            echo "Building with musl target..."
            cargo build \
              --release \
              --target x86_64-unknown-linux-musl \
              --offline \
              -j $NIX_BUILD_CORES

            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall

            mkdir -p $out/bin
            cp target/x86_64-unknown-linux-musl/release/nostrweet $out/bin/

            runHook postInstall
          '';

          doCheck = false; # Tests don't work well with static linking

          # Verify the binary is statically linked
          postInstall = ''
            echo "Checking if binary is statically linked..."
            file $out/bin/nostrweet || true
            # Strip the binary to reduce size
            ${pkgs.binutils}/bin/strip $out/bin/nostrweet || true
          '';
        };
      in
      {
        packages = {
          default = nostrweet;

          # Docker image output
          dockerImage = pkgs.dockerTools.buildImage {
            name = "nostrweet";
            tag = "latest";

            copyToRoot = pkgs.buildEnv {
              name = "image-root";
              paths = [
                nostrweet
                pkgs.bashInteractive
                pkgs.coreutils
                pkgs.cacert
              ];
              pathsToLink = [ "/bin" "/etc" ];
            };

            config = {
              Entrypoint = [ "/bin/nostrweet" ];
              Cmd = [];
              WorkingDir = "/data";
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "SYSTEM_CERTIFICATE_PATH=${pkgs.cacert}/etc/ssl/certs"
                "RUST_LOG=info"
                "NOSTRWEET_DATA_DIR=/data"
              ];
              Volumes = {
                "/data" = {};
              };
              ExposedPorts = {};
              Labels = {
                "org.opencontainers.image.source" = "https://github.com/douglaz/nostrweet";
                "org.opencontainers.image.description" = "Twitter to Nostr bridge daemon";
                "org.opencontainers.image.licenses" = "MIT";
              };
            };
          };
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            bashInteractive
            rustToolchain
            pkg-config
            pkgsStatic.stdenv.cc
            openssl
            openssl.dev
            just
            nixpkgs-fmt
            bc
            coreutils
            findutils
            gnugrep
            gnused
            gh
            nostr-rs-relay  # Nostr relay for integration tests
          ];

          CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}cc";
          CC_x86_64_unknown_linux_musl = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}cc";
          
          # Set OpenSSL environment variables
          OPENSSL_DIR = "${pkgs.openssl.dev}";
          OPENSSL_LIB_DIR = "${pkgs.openssl.out}/lib";
          OPENSSL_INCLUDE_DIR = "${pkgs.openssl.dev}/include";
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
          
          # Automatically configure Git hooks for code quality
          shellHook = ''
            # Set up Git hooks if not already configured
            if [ -d .git ] && [ -d .githooks ]; then
              current_hooks_path=$(git config core.hooksPath || echo "")
              if [ "$current_hooks_path" != ".githooks" ]; then
                echo "📎 Setting up Git hooks for code quality checks..."
                git config core.hooksPath .githooks
                echo "✅ Git hooks configured automatically!"
                echo "   • pre-commit: Checks code formatting"
                echo "   • pre-push: Runs formatting and clippy checks"
                echo ""
                echo "To disable: git config --unset core.hooksPath"
              fi
            fi
          '';
        };
      }
    );
}
