#!/usr/bin/env bash
set -euo pipefail

: "${PYTHON_PACKAGE_NAME:?}"
: "${MATURIN_FEATURES_TOML:?}"

sed_inplace() {
  if [[ "$(uname -s)" == Darwin* ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

release_tag="${RELEASE_TAG:-}"
if [[ -z "${release_tag}" && "${GITHUB_EVENT_NAME:-}" == "release" ]]; then
  release_tag="${GITHUB_REF_NAME:-}"
fi

if [[ -n "${release_tag}" ]]; then
  version="${release_tag#v}"
  sed_inplace "1,/^version = .*/s/^version = .*/version = \"${version}\"/" Cargo.toml
  sed_inplace "s/^version = .*/version = \"${version}\"/" pyproject.toml
fi

sed_inplace "s/^name = .*/name = \"${PYTHON_PACKAGE_NAME}\"/" pyproject.toml
sed_inplace "s/^features = .*/features = ${MATURIN_FEATURES_TOML}/" pyproject.toml

echo "Configured ${PYTHON_PACKAGE_NAME} with maturin features ${MATURIN_FEATURES_TOML}"
