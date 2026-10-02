{
  description = "workforest: isolated git worktrees for every piece of work";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      cargoToml = lib.importTOML ./Cargo.toml;
      rev = self.shortRev or self.dirtyShortRev or "unknown";

      build =
        pkgs:
        let
          craneLib = crane.mkLib pkgs;
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./tests
            ];
          };
          commonArgs = {
            inherit src;
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          workforest = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              doCheck = false;
              # The commit is part of `workforest --version`, so it only goes on
              # this derivation; the dependency build stays cached across commits.
              WORKFOREST_VERSION = "${cargoToml.package.version} (${rev})";
              postInstall = ''
                ln -s workforest "$out/bin/wf"
                install -Dm644 ${./plugin/skills/workforest/SKILL.md} \
                  "$out/share/workforest/skills/workforest/SKILL.md"
              '';
              meta = {
                description = cargoToml.package.description;
                homepage = "https://github.com/Nixotica/workforest";
                license = lib.licenses.mit;
                mainProgram = "workforest";
              };
            }
          );

          checks = {
            test = craneLib.cargoTest (
              commonArgs
              // {
                inherit cargoArtifacts;
                nativeBuildInputs = [ pkgs.git ];
              }
            );
            clippy = craneLib.cargoClippy (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              }
            );
            fmt = craneLib.cargoFmt { inherit src; };
          };

          shell = craneLib.devShell {
            packages = [
              pkgs.git
              pkgs.rust-analyzer
            ];
          };
        };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          inherit (build pkgs) workforest;
        in
        {
          inherit workforest;
          default = workforest;
        }
      );

      checks = forAllSystems (
        pkgs:
        (build pkgs).checks
        // {
          package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        }
      );

      devShells = forAllSystems (pkgs: {
        default = (build pkgs).shell;
      });

      overlays.default = final: _prev: {
        workforest = (build final).workforest;
      };

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
