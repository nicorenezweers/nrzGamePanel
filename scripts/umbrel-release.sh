#!/usr/bin/env bash
# Baut nrzGamePanel fuer umbrelOS (Linux amd64 + arm64), legt die fertigen Dateien in das
# oeffentliche Paket-Repo nrzGamePanel-umbrel (Zweig `umbrel`) und pusht den Tag `umbrel-<version>`.
# nrzGit traegt Paket und Version per nrzStore.json selbst im nrzStore ein.
#
#   ./scripts/umbrel-release.sh 1.0.0 "Kurze Beschreibung"
#
# Voraussetzungen: dioxus-cli 0.7 (dx), zig + cargo-zigbuild, Rust-Targets
# x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu und wasm32-unknown-unknown.
set -euo pipefail

VERSION="${1:?Version fehlt, z.B. 1.0.1}"
NOTES="${2:-nrzGamePanel ${VERSION}}"
TAG="umbrel-${VERSION}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG_REMOTE="${NRZGP_PKG_REMOTE:-$(git -C "${ROOT}" remote get-url origin | sed 's#/nrzGamePanel\.git$#/nrzGamePanel-umbrel.git#')}"
cd "${ROOT}"

if [ -n "$(git status --porcelain)" ] && [ "${ALLOW_DIRTY:-}" != 1 ]; then
  echo "Arbeitsverzeichnis nicht sauber - erst committen." >&2
  exit 1
fi

DX="${DX:-}"
for c in "${DX}" "$(command -v dx || true)" "${HOME}/.cargo/bin/dx"; do
  if [ -n "${c}" ] && "${c}" --version 2>/dev/null | grep -qi dioxus; then DX="${c}"; break; fi
  DX=""
done
[ -n "${DX}" ] || { echo "dioxus-cli (dx) nicht gefunden - cargo binstall dioxus-cli" >&2; exit 1; }
cargo zigbuild --help >/dev/null 2>&1 || { echo "cargo-zigbuild fehlt" >&2; exit 1; }

# Pfade ohne Leerzeichen (zig-Wrapper und C-Builds wie ring vertragen keine).
BUILD="${NRZGP_BUILD_DIR:-/tmp/nrzgp-build}"
export CARGO_ZIGBUILD_CACHE_DIR="${BUILD}/zigcache"
OUT="${BUILD}/out"
rm -rf "${OUT}" && mkdir -p "${OUT}" "${BUILD}/bin"
printf '#!/bin/sh\nexec zig ar "$@"\n' > "${BUILD}/bin/zig-ar"
printf '#!/bin/sh\nexec zig ranlib "$@"\n' > "${BUILD}/bin/zig-ranlib"
chmod +x "${BUILD}/bin/zig-ar" "${BUILD}/bin/zig-ranlib"
export AR="${BUILD}/bin/zig-ar" RANLIB="${BUILD}/bin/zig-ranlib"

echo "==> Oberflaeche (Dioxus/WASM)"
# Alte Assets frueherer Builds nicht mitliefern.
rm -rf "${BUILD}/webdist" "${BUILD}/wasm/dx/nrz-gamepanel-web/release/web/public"
(cd crates/web && CARGO_TARGET_DIR="${BUILD}/wasm" "${DX}" bundle --platform web --release --out-dir "${BUILD}/webdist")
test -f "${BUILD}/webdist/public/index.html"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "${OUT}/web.tar.gz" -C "${BUILD}/webdist/public" .

build() { # <rust-target> <arch>
  echo "==> Panel ${2} (${1})"
  CARGO_TARGET_DIR="${BUILD}/linux" cargo zigbuild --release -p nrz-gamepanel --target "${1}.2.31"
  gzip -9 -c "${BUILD}/linux/${1}/release/nrz-gamepanel" > "${OUT}/nrz-gamepanel-${2}.gz"
  # Speicher sparen: Zwischenstaende dieses Targets wegwerfen
  [ "${KEEP_BUILD:-}" = 1 ] || rm -rf "${BUILD}/linux/${1}/release/deps" "${BUILD}/linux/${1}/release/build"
}
build x86_64-unknown-linux-gnu amd64
build aarch64-unknown-linux-gnu arm64
(cd "${OUT}" && shasum -a 256 nrz-gamepanel-amd64.gz nrz-gamepanel-arm64.gz web.tar.gz > SHA256SUMS)

echo "==> Paket-Repo ${PKG_REMOTE}"
WT="$(mktemp -d)"
trap 'rm -rf "${WT}"' EXIT
if git ls-remote --exit-code "${PKG_REMOTE}" umbrel >/dev/null 2>&1; then
  git clone -q --depth 1 --branch umbrel "${PKG_REMOTE}" "${WT}"
else
  git init -q -b umbrel "${WT}" && git -C "${WT}" remote add origin "${PKG_REMOTE}"
fi
if git -C "${WT}" ls-remote --exit-code --tags origin "${TAG}" >/dev/null 2>&1; then
  echo "Tag ${TAG} gibt es schon." >&2
  exit 1
fi
find "${WT}" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp "${OUT}"/* "${WT}/"
cp scripts/umbrel-README.md "${WT}/README.md"
cp scripts/nrzStore.json "${WT}/nrzStore.json"
mkdir -p "${WT}/package"
cp -R umbrel/. "${WT}/package/"
# Version und RELEASE im Paket auf diesen Stand setzen
sed -i '' -e "s/^version: \".*\"/version: \"${VERSION}\"/" "${WT}/package/umbrel-app.yml"
sed -i '' -e "s/^RELEASE=\".*\"/RELEASE=\"${TAG}\"/" "${WT}/package/hooks/pre-start"
(
  cd "${WT}"
  git add -A
  git commit -qm "nrzGamePanel ${VERSION}: Umbrel-Paket" -m "${NOTES}"
  git tag -a "${TAG}" -m "${NOTES}"
  git push -q origin umbrel
  git push -q origin "${TAG}"
)
echo "==> ${TAG} veroeffentlicht. nrzGit aktualisiert den nrzStore (Protokoll: GET /api/v1/workflow)."
