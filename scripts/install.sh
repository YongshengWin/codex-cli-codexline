#!/bin/sh

set -eu

repository="YongshengWin/codex-cli-codexline"
version="${CODEXLINE_VERSION:-latest}"
install_dir="${CODEXLINE_INSTALL_DIR:-${HOME}/.local/bin}"
shim_dir="${XDG_DATA_HOME:-${HOME}/.local/share}/codexline/bin"

command -v curl >/dev/null 2>&1 || {
    echo "codexline: curl is required" >&2
    exit 1
}
command -v tar >/dev/null 2>&1 || {
    echo "codexline: tar is required" >&2
    exit 1
}

case "$(uname -s)" in
    Darwin) operating_system="apple-darwin" ;;
    Linux) operating_system="unknown-linux-musl" ;;
    *)
        echo "codexline: unsupported operating system; use the source installation instructions" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    arm64 | aarch64) architecture="aarch64" ;;
    x86_64 | amd64) architecture="x86_64" ;;
    *)
        echo "codexline: unsupported CPU architecture; use the source installation instructions" >&2
        exit 1
        ;;
esac

target="${architecture}-${operating_system}"
archive="codexline-${target}.tar.gz"
if [ "${version}" = "latest" ]; then
    download_base="https://github.com/${repository}/releases/latest/download"
else
    download_base="https://github.com/${repository}/releases/download/${version}"
fi

temporary_dir="$(mktemp -d 2>/dev/null || mktemp -d -t codexline)"
cleanup() {
    rm -rf "${temporary_dir}"
}
trap cleanup EXIT HUP INT TERM

echo "Downloading Codexline for ${target}..."
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
    "${download_base}/${archive}" --output "${temporary_dir}/${archive}"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
    "${download_base}/${archive}.sha256" --output "${temporary_dir}/${archive}.sha256"

(
    cd "${temporary_dir}"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum --check "${archive}.sha256"
    elif command -v shasum >/dev/null 2>&1; then
        shasum --algorithm 256 --check "${archive}.sha256"
    else
        echo "codexline: SHA-256 verification tool not found" >&2
        exit 1
    fi
    mkdir extracted
    tar -xzf "${archive}" -C extracted
)

destination="${install_dir}/codexline"
if [ -e "${destination}" ] && ! "${destination}" --version 2>/dev/null | grep -q '^codexline '; then
    echo "codexline: refusing to replace unrelated file at ${destination}" >&2
    exit 1
fi

mkdir -p "${install_dir}"
install -m 755 "${temporary_dir}/extracted/codexline" "${destination}.new"
mv -f "${destination}.new" "${destination}"

echo "Installed $("${destination}" --version) to ${destination}"
case ":${PATH}:" in
    *":${shim_dir}:"*":${install_dir}:"*) ;;
    *)
        echo
        echo "Add Codexline and its optional owned shim to your PATH, then open a new terminal:"
        echo "  export PATH=\"${shim_dir}:${install_dir}:\$PATH\""
        ;;
esac
echo
echo "Next: codexline config && codexline doctor && codexline"
