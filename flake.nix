{
  description = "workforest: isolated git worktrees for every piece of work";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    # Prebuilt Rust standard libraries for musl targets, for the static
    # binaries that releases ship; nixpkgs would build rustc from source.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
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

      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./src
          ./tests
          ./examples
        ];
      };

      # What every build of the package does once cargo has built it: the `wf`
      # name, shell completions for both names, and the skill.
      finish = pkgs: {
        # The commit is part of `workforest --version`, so it only goes on the
        # package; the dependency build stays cached across commits.
        WORKFOREST_VERSION = "${cargoToml.package.version} (${rev})";
        nativeBuildInputs = [ pkgs.installShellFiles ];
        postInstall = ''
          ln -s workforest "$out/bin/wf"
          for bin in workforest wf; do
            installShellCompletion --cmd "$bin" \
              --bash <("$out/bin/workforest" completions bash --bin "$bin") \
              --zsh <("$out/bin/workforest" completions zsh --bin "$bin") \
              --fish <("$out/bin/workforest" completions fish --bin "$bin")
          done
          install -Dm644 ${./plugin/skills/workforest/SKILL.md} \
            "$out/share/workforest/skills/workforest/SKILL.md"
        '';
        meta = {
          description = cargoToml.package.description;
          homepage = "https://github.com/Nixotica/workforest";
          license = lib.licenses.mit;
          mainProgram = "workforest";
        };
      };

      # The musl target that a Linux system's static build is for.
      muslTargets = {
        x86_64-linux = "x86_64-unknown-linux-musl";
        aarch64-linux = "aarch64-unknown-linux-musl";
      };

      # A statically linked build for `target`, a musl target, which runs on
      # any Linux whatever its libc: what releases ship. It fails unless the
      # binary really is static.
      buildStatic =
        pkgs: target:
        let
          toolchain = (pkgs.extend rust-overlay.overlays.default).rust-bin.stable.latest.minimal.override {
            targets = [ target ];
          };
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          args = {
            inherit src;
            strictDeps = true;
            doCheck = false;
            CARGO_BUILD_TARGET = target;
            CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
          };
          finished = finish pkgs;
        in
        craneLib.buildPackage (
          args
          // finished
          // {
            cargoArtifacts = craneLib.buildDepsOnly args;
            nativeBuildInputs = finished.nativeBuildInputs ++ [ pkgs.file ];
            doInstallCheck = true;
            installCheckPhase = ''
              file -L "$out/bin/workforest"
              file -L "$out/bin/workforest" | grep -Eq 'statically linked|static-pie linked'
            '';
          }
        );

      # A release tarball of `package`, built for `target`: the binary, `wf`,
      # shell completions, the skill and the license, under one directory
      # named after the version and target.
      tarball =
        pkgs: package: target:
        let
          name = "workforest-${cargoToml.package.version}-${target}";
        in
        pkgs.runCommand "${name}.tar.gz" { } ''
          mkdir ${name}
          cp -r ${package}/bin ${package}/share ${name}/
          cp ${./LICENSE} ${name}/LICENSE
          chmod -R u+w ${name}
          tar --sort=name --mtime=@1 --owner=0 --group=0 --numeric-owner -czf "$out" ${name}
        '';

      build =
        pkgs:
        let
          craneLib = crane.mkLib pkgs;
          commonArgs = {
            inherit src;
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          workforest = craneLib.buildPackage (
            commonArgs
            // finish pkgs
            // {
              inherit cargoArtifacts;
              doCheck = false;
            }
          );

          checks = {
            test = craneLib.cargoTest (
              commonArgs
              // {
                inherit cargoArtifacts;
                # The shell tests run each shell that is installed.
                nativeBuildInputs = [
                  pkgs.git
                  pkgs.bashInteractive
                  pkgs.zsh
                  pkgs.fish
                ];
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
              pkgs.bashInteractive
              pkgs.zsh
              pkgs.fish
            ];
          };
        };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          inherit (build pkgs) workforest;
          system = pkgs.stdenv.hostPlatform.system;
          target = muslTargets.${system} or null;
        in
        {
          inherit workforest;
          default = workforest;
        }
        // lib.optionalAttrs (target != null) rec {
          static = buildStatic pkgs target;
          release-tarball = tarball pkgs static target;
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
