{
  description = "silent, a borderless, fast file previewer for Nautilus on X11";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      forAllSystems = f: lib.genAttrs [ "x86_64-linux" ] (system: f nixpkgs.legacyPackages.${system});
      # Video and sound: playbin, the converters, the common demuxers and the
      # audio sinks, and FFmpeg's decoders for H.264, HEVC and AAC.
      gstPlugins =
        pkgs: with pkgs.gst_all_1; [
          gstreamer
          gst-plugins-base
          gst-plugins-good
          gst-libav
        ];
      # nixpkgs' libarchive has no lz4 of its own: it would run an lz4 program.
      libarchive =
        pkgs:
        pkgs.libarchive.overrideAttrs (old: {
          buildInputs = old.buildInputs ++ [ pkgs.lz4 ];
          doCheck = false;
        });
    in
    {
      packages = forAllSystems (pkgs: {
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.silent;

        silent = pkgs.rustPlatform.buildRustPackage {
          pname = "silent";
          version = (lib.importTOML ./Cargo.toml).package.version;
          # Only what the build reads, so a change to the docs or the site rebuilds nothing.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./vendor
              ./data
              ./assets/icon/linux
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = with pkgs; [
            pkg-config
            python3
            # glycin runs bubblewrap from the store, and binds /usr only where there is one.
            libglycin.patchVendorHook
            wrapGAppsNoGuiHook
          ];
          buildInputs =
            with pkgs;
            [
              fontconfig
              freetype
              glib
              libseccomp
              libxcb
              libxkbcommon
              libglycin.setupHook
              glycin-loaders
            ]
            ++ [ (libarchive pkgs) ]
            ++ gstPlugins pkgs;

          postInstall = ''
            install -Dm644 -t $out/share/dbus-1/services \
              data/org.gnome.NautilusPreviewer.service data/org.nemo.Preview.service
            substituteInPlace $out/share/dbus-1/services/*.service \
              --replace-fail /usr/bin/silent $out/bin/silent
            mkdir -p $out/share/icons
            cp -r assets/icon/linux/hicolor $out/share/icons/
          '';

          dontWrapGApps = true;
          # After the fixup has shrunk the RUNPATH to the linked libraries: wgpu loads Vulkan itself.
          postFixup = ''
            patchelf --add-rpath ${lib.makeLibraryPath [ pkgs.vulkan-loader ]} $out/bin/silent
            wrapProgram $out/bin/silent "''${gappsWrapperArgs[@]}"
          '';

          meta = {
            description = "A borderless, fast file previewer for Nautilus on X11";
            homepage = "https://github.com/crazyboycjr/silent";
            license = lib.licenses.asl20;
            mainProgram = "silent";
            platforms = [ "x86_64-linux" ];
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.silent ];
          packages = with pkgs; [
            bubblewrap
            clippy
            mold
            rust-analyzer
            rustfmt
          ];
          # What the package's wrapper gives the installed binary, for `cargo run`;
          # the GStreamer plugins' path comes from their setup hook.
          env = {
            LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.vulkan-loader ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
          shellHook = ''
            export XDG_DATA_DIRS=${pkgs.glycin-loaders}/share:''${XDG_DATA_DIRS:-/usr/local/share:/usr/share}
          '';
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
