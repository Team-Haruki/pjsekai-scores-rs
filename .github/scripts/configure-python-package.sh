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

# The version is never rewritten here: Cargo.toml and pyproject.toml are bumped in a PR
# before tagging, and release-gate checks the tag against them.
sed_inplace "s/^name = .*/name = \"${PYTHON_PACKAGE_NAME}\"/" pyproject.toml
sed_inplace "s/^features = .*/features = ${MATURIN_FEATURES_TOML}/" pyproject.toml

echo "Configured ${PYTHON_PACKAGE_NAME} with maturin features ${MATURIN_FEATURES_TOML}"
