{
  pkgs ? import <nixpkgs> { },
}:
pkgs.mkShell {
  packages = with pkgs; [
    cargo
    rust-analyzer
    rustc
    rustfmt
  ];
}
