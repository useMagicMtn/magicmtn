{
  description = "MagicMtn — Distributed compute for agents";

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
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      mkMagicmtn =
        pkgs:
        let
          inherit (pkgs) lib stdenv;
          fs = lib.fileset;
          rustToolchain = (rust-overlay.lib.mkRustBin { } pkgs.buildPackages).fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          cliManifest = builtins.fromTOML (builtins.readFile ./cli/Cargo.toml);
        in
        rustPlatform.buildRustPackage {
          pname = "magicmtn";
          inherit (cliManifest.package) version;

          src = fs.toSource {
            root = ./.;
            fileset = fs.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./assets
              ./cli
              ./core
              ./web-ui
              ./local
              ./sdk/rust
              ./interfaces/pi
              ./interfaces/opencode
              ./registry/providers.json
              ./release/cli-trust.json
              ./runtime/errors.js
              ./runtime/error-worker.js
              ./scripts/install-public.sh
              ./tests/fixtures
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;
          buildNoDefaultFeatures = true;
          cargoBuildFlags = [ "-p" "magicmtn-cli" "--bin" "magic" ];
          cargoTestFlags = [ "-p" "magicmtn-cli" ];

          nativeBuildInputs = [
            pkgs.installShellFiles
            pkgs.pkg-config
          ];
          buildInputs = [ pkgs.sqlite ];
          nativeCheckInputs = [ pkgs.python3 ];
          preCheck = ''export HOME=$(mktemp -d)'';

          postInstall = lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
            installShellCompletion --cmd magic \
              --bash <($out/bin/magic completion bash --print) \
              --zsh <($out/bin/magic completion zsh --print) \
              --fish <($out/bin/magic completion fish --print)
          '';

          doInstallCheck = true;
          nativeInstallCheckInputs = [ pkgs.versionCheckHook ];
          versionCheckProgram = "${placeholder "out"}/bin/magic";

          meta = {
            description = "Command-line client for MagicMtn distributed compute and agent sandboxes";
            homepage = "https://github.com/useMagicMtn/magicmtn";
            license = lib.licenses.asl20;
            mainProgram = "magic";
            platforms = lib.platforms.unix;
          };
        };
    in
    {
      packages = forAllSystems (system: rec {
        magicmtn = mkMagicmtn nixpkgs.legacyPackages.${system};
        default = magicmtn;
      });

      overlays.default = final: _prev: {
        magicmtn = mkMagicmtn final;
      };
    };
}
