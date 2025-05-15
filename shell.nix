{
  pkgs ? import <nixpkgs> { },
}:
pkgs.mkShell {
  packages = with pkgs; [
    cargo
    clippy
    openssl
    pkg-config
    rust-analyzer
    rustc
    rustfmt
  ];
}
