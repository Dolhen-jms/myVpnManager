# Environnement de développement :
#   nix-shell --run 'cargo build --release'
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  nativeBuildInputs = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt ];
  buildInputs = [ pkgs.openconnect ];
}
