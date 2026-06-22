{
  fetchCrate,
  rustPlatform,
}:

rustPlatform.buildRustPackage rec {
  pname = "hotpath";
  version = "0.16.1";

  src = fetchCrate {
    inherit pname version;
    hash = "sha256-o/9wqlq2dXy1j23c4XdKgDROuUVpy7CdwRz3SPCv3Ok=";
  };

  cargoLock = {
    lockFile = "${src}/Cargo.lock";
  };

  buildFeatures = [ "tui" ];
  doCheck = false;
}
