{
  pkgs ? import <nixpkgs> { },
}:
pkgs.mkShell {
  packages = with pkgs; [
    cargo
    openssl
    pkg-config
    rust-analyzer
    rustc
    rustfmt
  ];
}
