{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs, ... }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [ "aarch64-darwin" "x86_64-linux" ];
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy
              pkgs.rustfmt
              # bin/coverage and `just mutants`.
              pkgs.grcov
              pkgs.jq
              pkgs.bc
              pkgs.just
              pkgs.cargo-mutants
              # hegeltest runs its engine through uv.
              pkgs.uv
              # Loading frork-lua and compiling Fennel binaries against it.
              pkgs.lua5_4
              pkgs.lua54Packages.fennel
            ];

            # There is no rustup llvm-tools component here, so bin/coverage
            # points grcov at the LLVM that rustc was built with.
            GRCOV_LLVM_PATH = "${pkgs.rustc.llvmPackages.llvm}/bin";
            RANGER_DEFAULT_BACKLOG = "frork";
          };
        });

      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib;

          # The CalVer shape CI releases with, from the commit the flake was
          # built from rather than build.rs's `date` and `jj`.
          version =
            let
              d = self.lastModifiedDate;
              s = lib.substring;
            in
            "${s 0 4 d}-${s 4 2 d}-${s 6 2 d}+${self.shortRev or self.dirtyShortRev or "unknown"}";

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              # Carries the macOS link flags frork-lua's cdylib needs.
              ./.cargo/config.toml
              ./frork-cli
              ./frork-lib
              ./frork-lua
            ];
          };

          workspaceCrate = args: pkgs.rustPlatform.buildRustPackage ({
            inherit version src;
            cargoLock.lockFile = ./Cargo.lock;
            FRORK_VERSION = version;
          } // args);
        in
        {
          frork = workspaceCrate {
            pname = "frork";
            cargoBuildFlags = [ "-p" "frork-cli" ];
            # frork-lib's tests need network access for hegeltest and run real
            # git, so only the CLI's own tests run in the sandbox.
            cargoTestFlags = [ "-p" "frork-cli" ];
            meta.mainProgram = "frork";
          };

          frork-lua = workspaceCrate {
            pname = "frork-lua";
            cargoBuildFlags = [ "-p" "frork" ];
            # A module build leaves Lua's symbols for the host to provide, so a
            # test binary would not link.
            doCheck = false;
            # Where Lua's default cpath looks for `require("frork")`.
            postInstall = ''
              mkdir -p $out/lib/lua/5.4
              ln -s ../../libfrork${pkgs.stdenv.hostPlatform.extensions.sharedLibrary} $out/lib/lua/5.4/frork.so
            '';
          };

          default = self.packages.${system}.frork;
        });
    };
}
