#!/usr/bin/env bash
# Download static FFmpeg builds for Tauri sidecar bundling.
#
# Places binaries in src-tauri/binaries/ffmpeg-{target-triple}
# which Tauri's externalBin mechanism picks up at build time.
#
# Usage:
#   ./scripts/download-ffmpeg.sh          # auto-detect current platform
#   ./scripts/download-ffmpeg.sh macos    # force macOS download
#   ./scripts/download-ffmpeg.sh windows  # force Windows download

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARIES_DIR="${PROJECT_ROOT}/apps/desktop/src-tauri/binaries"

MACOS_AARCH64_URL="https://www.osxexperts.net/ffmpeg7arm.zip"
MACOS_X86_64_URL="https://evermeet.cx/ffmpeg/getrelease/zip"
WINDOWS_URL="https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"

mkdir -p "${BINARIES_DIR}"

download_macos_aarch64() {
    echo "Downloading FFmpeg for macOS (aarch64)..."
    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "${tmpdir}"' EXIT

    curl -L -o "${tmpdir}/ffmpeg.zip" "${MACOS_AARCH64_URL}"
    unzip -o "${tmpdir}/ffmpeg.zip" -d "${tmpdir}"

    local ffmpeg_bin
    ffmpeg_bin="$(find "${tmpdir}" -name 'ffmpeg' -type f | head -1)"
    if [[ -z "${ffmpeg_bin}" ]]; then
        echo "Error: ffmpeg binary not found in downloaded archive"
        exit 1
    fi

    cp "${ffmpeg_bin}" "${BINARIES_DIR}/ffmpeg-aarch64-apple-darwin"
    chmod +x "${BINARIES_DIR}/ffmpeg-aarch64-apple-darwin"
    echo "Installed: ${BINARIES_DIR}/ffmpeg-aarch64-apple-darwin"
}

download_macos_x86_64() {
    echo "Downloading FFmpeg for macOS (x86_64)..."
    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "${tmpdir}"' EXIT

    curl -L -o "${tmpdir}/ffmpeg.zip" "${MACOS_X86_64_URL}"
    unzip -o "${tmpdir}/ffmpeg.zip" -d "${tmpdir}"

    local ffmpeg_bin
    ffmpeg_bin="$(find "${tmpdir}" -name 'ffmpeg' -type f | head -1)"
    if [[ -z "${ffmpeg_bin}" ]]; then
        echo "Error: ffmpeg binary not found in downloaded archive"
        exit 1
    fi

    cp "${ffmpeg_bin}" "${BINARIES_DIR}/ffmpeg-x86_64-apple-darwin"
    chmod +x "${BINARIES_DIR}/ffmpeg-x86_64-apple-darwin"
    echo "Installed: ${BINARIES_DIR}/ffmpeg-x86_64-apple-darwin"
}

download_windows() {
    echo "Downloading FFmpeg for Windows (x86_64)..."
    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "${tmpdir}"' EXIT

    curl -L -o "${tmpdir}/ffmpeg.zip" "${WINDOWS_URL}"
    unzip -o "${tmpdir}/ffmpeg.zip" -d "${tmpdir}"

    local ffmpeg_bin
    ffmpeg_bin="$(find "${tmpdir}" -name 'ffmpeg.exe' -type f | head -1)"
    if [[ -z "${ffmpeg_bin}" ]]; then
        echo "Error: ffmpeg.exe not found in downloaded archive"
        exit 1
    fi

    cp "${ffmpeg_bin}" "${BINARIES_DIR}/ffmpeg-x86_64-pc-windows-msvc.exe"
    echo "Installed: ${BINARIES_DIR}/ffmpeg-x86_64-pc-windows-msvc.exe"
}

PLATFORM="${1:-auto}"

if [[ "${PLATFORM}" == "auto" ]]; then
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64)  PLATFORM="macos-aarch64" ;;
        Darwin-x86_64) PLATFORM="macos-x86_64" ;;
        MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
        *)
            echo "Unknown platform: $(uname -s)-$(uname -m)"
            echo "Usage: $0 [macos|macos-aarch64|macos-x86_64|windows]"
            exit 1
            ;;
    esac
fi

case "${PLATFORM}" in
    macos)
        ARCH="$(uname -m)"
        if [[ "${ARCH}" == "arm64" ]]; then
            download_macos_aarch64
        else
            download_macos_x86_64
        fi
        ;;
    macos-aarch64) download_macos_aarch64 ;;
    macos-x86_64)  download_macos_x86_64 ;;
    windows)       download_windows ;;
    *)
        echo "Unknown platform: ${PLATFORM}"
        echo "Usage: $0 [macos|macos-aarch64|macos-x86_64|windows]"
        exit 1
        ;;
esac

echo ""
echo "FFmpeg sidecar ready. Build with: pnpm tauri build"
