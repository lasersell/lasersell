#!/usr/bin/env bash
set -euo pipefail

# Inputs:
#   REPO_DIR: path to checked out gh-pages working tree (required)
#   DEB_PATH: path or glob to .deb file(s) to publish (required)
#   DIST (e.g. stable)
#   COMPONENT (e.g. main)
#   ARCH (e.g. amd64) [legacy]
#   ARCHES (e.g. "amd64 arm64")
#   PACKAGE_NAME (e.g. lasersell)
#   GPG_KEY_FPR: fingerprint of the signing key (required, already imported in CI)
#   GPG_PASSPHRASE: optional passphrase for the signing key

DIST="${DIST:-stable}"
COMPONENT="${COMPONENT:-main}"
ARCH="${ARCH:-}"
ARCHES="${ARCHES:-}"
PACKAGE_NAME="${PACKAGE_NAME:-lasersell}"

if [ -z "${ARCHES}" ]; then
  if [ -n "${ARCH}" ]; then
    ARCHES="${ARCH}"
  else
    ARCHES="amd64"
  fi
fi

if [ -z "${ARCH}" ]; then
  ARCH="${ARCHES%% *}"
fi

: "${REPO_DIR:?REPO_DIR is required}"
: "${DEB_PATH:?DEB_PATH is required}"
: "${GPG_KEY_FPR:?GPG_KEY_FPR is required}"

if [ -z "${PACKAGE_NAME}" ]; then
  echo "Error: PACKAGE_NAME is required." >&2
  exit 1
fi

ORIGIN="${ORIGIN:-Lasersell}"
LABEL="${LABEL:-Lasersell}"
SUITE="${SUITE:-${DIST}}"
CODENAME="${CODENAME:-${DIST}}"
DESCRIPTION="${DESCRIPTION:-Lasersell APT Repository}"

PACKAGE_LETTER="${PACKAGE_NAME:0:1}"
PACKAGE_LETTER="${PACKAGE_LETTER,,}"

POOL_REL_DIR="pool/${COMPONENT}/${PACKAGE_LETTER}/${PACKAGE_NAME}"
POOL_DIR="${REPO_DIR}/${POOL_REL_DIR}"
DIST_REL_DIR="dists/${DIST}"
DIST_DIR="${REPO_DIR}/${DIST_REL_DIR}"

read -r -a arch_list <<< "${ARCHES}"
if [ "${#arch_list[@]}" -eq 0 ]; then
  echo "Error: ARCHES resolved to an empty list." >&2
  exit 1
fi

ARCHES_LIST="${arch_list[*]}"

mkdir -p "${POOL_DIR}" "${DIST_DIR}"
for arch in "${arch_list[@]}"; do
  bin_rel_dir="${DIST_REL_DIR}/${COMPONENT}/binary-${arch}"
  bin_dir="${REPO_DIR}/${bin_rel_dir}"
  mkdir -p "${bin_dir}"
done

# Expand glob(s) for deb files safely.
shopt -s nullglob
deb_files=( ${DEB_PATH} )
shopt -u nullglob

if [ "${#deb_files[@]}" -eq 0 ]; then
  echo "Error: DEB_PATH did not match any .deb files: ${DEB_PATH}" >&2
  exit 1
fi

for deb_file in "${deb_files[@]}"; do
  if [[ "${deb_file}" != *.deb ]]; then
    echo "Error: non-.deb file matched: ${deb_file}" >&2
    exit 1
  fi
done

# Copy .deb into pool
cp -f "${deb_files[@]}" "${POOL_DIR}/"

# Generate Packages / Packages.gz and Release file with repo-relative paths.
pushd "${REPO_DIR}" >/dev/null
for arch in "${arch_list[@]}"; do
  bin_rel_dir="${DIST_REL_DIR}/${COMPONENT}/binary-${arch}"
  dpkg-scanpackages --arch "${arch}" "pool/${COMPONENT}" > "${bin_rel_dir}/Packages"
  gzip -9c "${bin_rel_dir}/Packages" > "${bin_rel_dir}/Packages.gz"
done

apt-ftparchive \
  -o APT::FTPArchive::Release::Origin="${ORIGIN}" \
  -o APT::FTPArchive::Release::Label="${LABEL}" \
  -o APT::FTPArchive::Release::Suite="${SUITE}" \
  -o APT::FTPArchive::Release::Codename="${CODENAME}" \
  -o APT::FTPArchive::Release::Components="${COMPONENT}" \
  -o APT::FTPArchive::Release::Architectures="${ARCHES_LIST}" \
  -o APT::FTPArchive::Release::Description="${DESCRIPTION}" \
  release "${DIST_REL_DIR}" > "${DIST_REL_DIR}/Release"
popd >/dev/null

# Sign Release -> InRelease and Release.gpg
passphrase_file=""
cleanup() {
  if [ -n "${passphrase_file}" ] && [ -f "${passphrase_file}" ]; then
    rm -f "${passphrase_file}"
  fi
}
trap cleanup EXIT

gpg_args=(--batch --yes --pinentry-mode loopback --local-user "${GPG_KEY_FPR}")

if [ -n "${GPG_PASSPHRASE:-}" ]; then
  passphrase_file="$(mktemp)"
  chmod 600 "${passphrase_file}"
  printf '%s' "${GPG_PASSPHRASE}" > "${passphrase_file}"
  gpg_args+=(--passphrase-file "${passphrase_file}")
fi

gpg "${gpg_args[@]}" --clearsign -o "${DIST_DIR}/InRelease" "${DIST_DIR}/Release"
gpg "${gpg_args[@]}" -abs -o "${DIST_DIR}/Release.gpg" "${DIST_DIR}/Release"
