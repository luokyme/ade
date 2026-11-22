#!/usr/bin/env sh
set -eu

# Downloads a Ade release from GitHub and unpacks it into ~/.local/.
# Optional: ADE_VERSION=1.2.3 picks a release, ADE_BUNDLE_PATH installs a local tarball,
# ADE_BUNDLE_DIR installs a staged Linux application directory.
# The ZED_ names of these variables still work for older instructions.

main() {
    platform="$(uname -s)"
    arch="$(uname -m)"
    channel="${ADE_CHANNEL:-${ZED_CHANNEL:-stable}}"
    repo="luokyme/ade"
    version="${ADE_VERSION:-${ZED_VERSION:-latest}}"
    bundle_path="${ADE_BUNDLE_PATH:-${ZED_BUNDLE_PATH:-}}"
    bundle_dir="${ADE_BUNDLE_DIR:-}"
    if [ "$version" = "latest" ]; then
        download_base="https://github.com/$repo/releases/latest/download"
    else
        download_base="https://github.com/$repo/releases/download/v$version"
    fi
    # Use TMPDIR if available (for environments with non-standard temp directories)
    if [ -n "${TMPDIR:-}" ] && [ -d "${TMPDIR}" ]; then
        temp="$(mktemp -d "$TMPDIR/ade-XXXXXX")"
    else
        temp="$(mktemp -d "/tmp/ade-XXXXXX")"
    fi

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    case "$platform-$arch" in
        macos-arm64* | linux-arm64* | linux-aarch64)
            arch="aarch64"
            ;;
        macos-x86* | linux-x86*)
            arch="x86_64"
            ;;
        *)
            echo "Unsupported platform or architecture"
            exit 1
            ;;
    esac

    if [ "$platform" = linux ] && { [ -n "$bundle_dir" ] || [ -n "$bundle_path" ]; }; then
        :
    elif command -v curl >/dev/null 2>&1; then
        curl () {
            command curl -fL "$@"
        }
    elif command -v wget >/dev/null 2>&1; then
        curl () {
            wget -O- "$@"
        }
    else
        echo "Could not find 'curl' or 'wget' in your path"
        exit 1
    fi

    "$platform" "$@"

    if [ "$(command -v ade)" = "$HOME/.local/bin/ade" ]; then
        echo "Ade has been installed. Run with 'ade'"
    else
        echo "To run Ade from your terminal, you must add ~/.local/bin to your PATH"
        echo "Run:"

        case "$SHELL" in
            *zsh)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
                echo "   source ~/.zshrc"
                ;;
            *fish)
                echo "   fish_add_path -U $HOME/.local/bin"
                ;;
            *)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
                echo "   source ~/.bashrc"
                ;;
        esac

        echo "To run Ade now, '~/.local/bin/ade'"
    fi
}

linux() {
    trap 'rm -rf "$temp"' EXIT
    appid="me.farshed.Ade"
    if [ -z "$bundle_dir" ]; then
        if [ -n "$bundle_path" ]; then
            cp "$bundle_path" "$temp/ade-linux-$arch.tar.gz"
        else
            echo "Downloading Ade version: $version"
            curl "$download_base/ade-linux-$arch.tar.gz" > "$temp/ade-linux-$arch.tar.gz"
        fi
        tar -xzf "$temp/ade-linux-$arch.tar.gz" -C "$temp"
        bundle_dir="$temp/ade.app"
        if [ ! -d "$bundle_dir" ] && [ -d "$temp/ade-$channel.app" ]; then
            bundle_dir="$temp/ade-$channel.app"
        fi
    fi
    if [ ! -x "$bundle_dir/libexec/ade-editor" ] || [ ! -x "$bundle_dir/bin/ade" ]; then
        echo "Invalid Ade bundle: editor or CLI is missing"
        exit 1
    fi
    desktop_source="$bundle_dir/share/applications/$appid.desktop"
    if [ ! -f "$desktop_source" ]; then
        desktop_source="$bundle_dir/share/applications/me.farshed.Ade-Dev.desktop"
    fi
    cp "$desktop_source" "$temp/Ade.desktop"

    mkdir -p "$HOME/.local"
    rm -rf "$HOME/.local/ade.app"
    mv "$bundle_dir" "$HOME/.local/ade.app"

    ade_editor="$HOME/.local/ade.app/libexec/ade-editor"
    if [ -f "$ade_editor" ] && command -v ldd >/dev/null 2>&1; then
        missing="$(ldd "$ade_editor" 2>/dev/null | sed -n 's/^[[:space:]]*\(.*\) => not found$/\1/p')"
        if [ -n "$missing" ]; then
            echo "Warning: your system is missing libraries that Ade needs:"
            echo "$missing" | sed 's/^/    /'
            echo "Install them with your package manager, or Ade will fail to start."
        fi
    fi

    # Setup ~/.local directories
    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"

    # A user launcher outside the replaced bundle can preserve local settings across installs.
    ade_cli="$HOME/.local/ade.app/bin/ade"
    user_launcher="$HOME/.local/libexec/ade-launcher"
    if [ -f "$user_launcher" ] && [ -x "$user_launcher" ]; then
        ade_cli="$user_launcher"
    fi
    ln -sf "$ade_cli" "$HOME/.local/bin/ade"

    # Copy .desktop file
    desktop_file_path="$HOME/.local/share/applications/${appid}.desktop"
    cp "$temp/Ade.desktop" "${desktop_file_path}"
    sed -i "s/^Name=Ade Dev$/Name=Ade/" "${desktop_file_path}"
    sed -i "s|Icon=ade|Icon=$HOME/.local/ade.app/share/icons/hicolor/512x512/apps/ade.png|g" "${desktop_file_path}"
    sed -i "s|Exec=ade|Exec=$ade_cli|g" "${desktop_file_path}"
    rm -f "$HOME/.local/share/applications/me.farshed.Ade-Dev.desktop"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$HOME/.local/share/applications"
    fi
}

macos() {
    echo "Downloading Ade version: $version"
    curl "$download_base/Ade-$arch.dmg" > "$temp/Ade-$arch.dmg"
    hdiutil attach -quiet "$temp/Ade-$arch.dmg" -mountpoint "$temp/mount"
    app="$(cd "$temp/mount/"; echo *.app)"
    echo "Installing $app"
    if [ -d "/Applications/$app" ]; then
        echo "Removing existing $app"
        rm -rf "/Applications/$app"
    fi
    ditto "$temp/mount/$app" "/Applications/$app"
    hdiutil detach -quiet "$temp/mount"

    mkdir -p "$HOME/.local/bin"
    # Link the binary
    ln -sf "/Applications/$app/Contents/MacOS/cli" "$HOME/.local/bin/ade"
}

main "$@"
