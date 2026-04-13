{
  stdenv,
  python3,
  gh,
  basedpyright,
  makeWrapper,
}:
stdenv.mkDerivation {
  name = "bombadil-release";
  src = ./.;
  nativeBuildInputs = [
    basedpyright
    makeWrapper
  ];
  doCheck = true;
  checkPhase = ''
    basedpyright .
  '';
  installPhase = ''
    mkdir -p $out/bin $out/lib/bombadil-release
    cp *.py $out/lib/bombadil-release/
    makeWrapper ${python3}/bin/python3 $out/bin/release \
      --add-flags "$out/lib/bombadil-release/release.py" \
      --prefix PATH : ${gh}/bin
  '';
}
