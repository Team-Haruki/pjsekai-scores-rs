#!/usr/bin/env bash
# Builds the deprecated `pjsekai-scores-rs-skia-image` shim (sdist + py3-none-any wheel)
# into the given directory and checks that it is metadata only.
set -euo pipefail

out="$(mkdir -p "${1:?usage: build-shim.sh <out-dir>}" && cd "$1" && pwd)"
shim=python/skia-image-shim

python -m pip install --quiet --disable-pip-version-check build==1.6.1 twine==7.0.0
python -m build --outdir "$out" "$shim"
python -m twine check --strict "$out"/pjsekai_scores_rs_skia_image-*

wheel=$(ls "$out"/pjsekai_scores_rs_skia_image-*-py3-none-any.whl)
python - "$wheel" <<'PY'
import sys, zipfile
names = zipfile.ZipFile(sys.argv[1]).namelist()
code = [n for n in names if ".dist-info/" not in n]
assert not code, f"the shim must not ship modules: {code}"
meta = zipfile.ZipFile(sys.argv[1]).read(next(n for n in names if n.endswith("METADATA"))).decode()
assert "Requires-Dist: pjsekai-scores-rs>=0.6.0" in meta, meta
print(f"shim OK: {sys.argv[1]}")
PY
