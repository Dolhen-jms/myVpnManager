# Paquet Nix de myVpnManager.
#   nix-build          → ./result/bin/myvpnmanager
#   nix-env -f . -i    → installe dans le profil
# ou, dans configuration.nix :
#   environment.systemPackages = [ (pkgs.callPackage /chemin/vers/myvpnmanager { }) ];
{ pkgs ? import <nixpkgs> { } }:
let
  cargo = (pkgs.lib.importTOML ./Cargo.toml).package;
in
pkgs.rustPlatform.buildRustPackage {
  pname = cargo.name;
  inherit (cargo) version;
  src = pkgs.lib.cleanSourceWith {
    src = ./.;
    # vpn.sh (ancien script) contient un mot de passe en clair : jamais dans le /nix/store
    filter = path: type:
      let n = baseNameOf path; in
      !(pkgs.lib.hasPrefix "result" n) && n != "target" && n != "vpn.sh" && !(pkgs.lib.hasSuffix ".toml.tmp" n);
  };
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.makeWrapper ];
  # le rendu de l'interface lit la config de l'utilisateur : pas dans le bac à sable
  checkFlags = [ "--skip=ui::tests" ];
  postInstall = ''
    install -Dm644 data/myvpnmanager.desktop $out/share/applications/myvpnmanager.desktop
    # openconnect fourni s'il n'est pas installé ; celui du système reste prioritaire.
    # sudo, kitty et hyprctl : ceux du système (sudo doit être le wrapper setuid de NixOS).
    wrapProgram $out/bin/myvpnmanager --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.openconnect ]}
  '';
  meta = {
    description = cargo.description;
    mainProgram = "myvpnmanager";
    platforms = pkgs.lib.platforms.linux;
  };
}
