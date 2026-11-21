#!/bin/bash
# Double-click in Finder to build "Tagcast Lite.app" for this Mac.
cd "$(dirname "$0")" || exit 1
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
mkdir -p target
log=target/build.log
finish() {
    echo
    read -r -n 1 -p "Press any key to close this window."
    exit "$1"
}

{
    echo "== Building Tagcast Lite =="
    date
    if ! xcode-select -p >/dev/null 2>&1; then
        echo "Apple's Command Line Tools are missing. A dialog opens to install them."
        echo "When that has finished, double-click this script again."
        xcode-select --install
        exit 2
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "Rust is not installed. Installing it with rustup (https://rustup.rs)…"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal || exit 2
        . "$HOME/.cargo/env"
    fi
    version=$(rustc --version | awk '{print $2}')
    major=${version%%.*}
    minor=$(echo "$version" | cut -d. -f2)
    if [ "$major" -lt 1 ] || { [ "$major" -eq 1 ] && [ "$minor" -lt 95 ]; }; then
        if command -v rustup >/dev/null 2>&1; then
            echo "Rust $version is too old. Updating…"
            rustup update stable || exit 3
        else
            echo "Rust $version is too old and rustup is missing. Update Rust to 1.95 or newer."
            exit 3
        fi
    fi
    rustc --version
    scripts/make-macos-app.sh
} 2>&1 | tee "$log"
status=${PIPESTATUS[0]}

if [ "$status" -eq 0 ]; then
    echo "BUILD OK" >> "$log"
    echo
    echo "Done: target/release/Tagcast Lite.app"
    open -R "target/release/Tagcast Lite.app"
else
    echo "BUILD FAILED ($status)" >> "$log"
    echo
    echo "The build failed. Claude can read target/build.log."
fi
finish "$status"
