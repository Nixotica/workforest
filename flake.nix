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
    # Only for the check that evaluates the Home Manager module.
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
      home-manager,
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

      # The target each release binary is built for: static musl on Linux,
      # and the native target on Apple Silicon.
      releaseTargets = {
        x86_64-linux = "x86_64-unknown-linux-musl";
        aarch64-linux = "aarch64-unknown-linux-musl";
        aarch64-darwin = "aarch64-apple-darwin";
      };

      # The macOS build that releases ship, which must run on a Mac without
      # Nix: it fails if the binary links anything outside the system's
      # libraries, as Nix-built Darwin binaries can, such as libiconv from
      # /nix/store.
      buildSystemLinked =
        pkgs:
        (build pkgs).workforest.overrideAttrs (old: {
          doInstallCheck = true;
          nativeInstallCheckInputs = (old.nativeInstallCheckInputs or [ ]) ++ [ pkgs.cctools ];
          installCheckPhase = ''
            otool -L "$out/bin/workforest"
            if otool -L "$out/bin/workforest" | tail -n +2 | grep -Ev '^[[:space:]]*(/usr/lib/|/System/)'; then
              echo "links libraries outside the system's" >&2
              exit 1
            fi
          '';
        });

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

      # The Home Manager module in a minimal configuration, with everything
      # on: the files it writes must say what the options asked for.
      homeManagerCheck =
        pkgs:
        let
          inherit (pkgs.stdenv.hostPlatform) isDarwin isLinux;
          home = home-manager.lib.homeManagerConfiguration {
            inherit pkgs;
            modules = [
              self.homeManagerModules.default
              {
                home = {
                  username = "test";
                  homeDirectory = if isDarwin then "/Users/test" else "/home/test";
                  stateVersion = "25.05";
                };
                programs.bash.enable = true;
                programs.zsh.enable = true;
                programs.fish.enable = true;
                programs.workforest = {
                  enable = true;
                  settings = {
                    repos = "~/code";
                    cache.link_min = 4096;
                  };
                  installClaudeSkill = true;
                  fire.enable = isLinux;
                };
              }
            ];
          };
          files = home.config.home-files;
        in
        pkgs.runCommand "home-manager-module" { } (
          ''
            set -x
            grep -qx 'repos = "~/code"' ${files}/.config/workforest/config.toml
            grep -qx 'link_min = 4096' ${files}/.config/workforest/config.toml
            test -f ${files}/.claude/skills/workforest/SKILL.md
            grep -q 'shell-init bash' ${files}/.bashrc
            grep -q 'shell-init zsh' ${files}/.zshrc
            grep -q 'shell-init fish | source' ${files}/.config/fish/config.fish
          ''
          + lib.optionalString isLinux ''
            grep -q 'ExecStart=.*/bin/workforest fire --yes' ${files}/.config/systemd/user/workforest-fire.service
            grep -q 'OnCalendar=daily' ${files}/.config/systemd/user/workforest-fire.timer
          ''
          + ''
            touch "$out"
          ''
        );

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
          target = releaseTargets.${system} or null;
        in
        {
          inherit workforest;
          default = workforest;
        }
        // lib.optionalAttrs (target != null) rec {
          release-binary =
            if pkgs.stdenv.hostPlatform.isDarwin then buildSystemLinked pkgs else buildStatic pkgs target;
          release-tarball = tarball pkgs release-binary target;
        }
      );

      checks = forAllSystems (
        pkgs:
        (build pkgs).checks
        // {
          package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
          home-manager-module = homeManagerCheck pkgs;
        }
      );

      homeManagerModules.default = import ./nix/home-manager.nix self;

      devShells = forAllSystems (pkgs: {
        default = (build pkgs).shell;
      });

      overlays.default = final: _prev: {
        workforest = (build final).workforest;
      };

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
