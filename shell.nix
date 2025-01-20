{
  description = "tmux-booster development environment";

  inputs = {
    # nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    { systems, nixpkgs, ... }@inputs:
    let
      eachSystem = f: nixpkgs.lib.genAttrs (import systems) (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = eachSystem (pkgs: {
        default = pkgs.mkShell {
          name = "tmux-booster";
          buildInputs = with pkgs; [
            rustc
            cargo
            rustfmt
            rustPackages.clippy
          ];
          shellHook = ''
            echo "node `${pkgs.nodejs}/bin/node --version`"
          '';
        };
      });
    };
}
