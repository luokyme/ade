#!/usr/bin/env sh
set -eu

# Uninstalls Ade that was installed using the install.sh script

check_remaining_installations() {
    platform="$(uname -s)"
    if [ "$platform" = "Darwin" ]; then
        # Check for any Ade variants in /Applications
        remaining=$(ls -d /Applications/Ade*.app 2>/dev/null | wc -l)
        [ "$remaining" -eq 0 ]
    else
        # Check for any Ade variants in ~/.local
        remaining=$(ls -d "$HOME/.local/ade"*.app 2>/dev/null | wc -l)
        [ "$remaining" -eq 0 ]
    fi
}

prompt_remove_preferences() {
    printf "Do you want to keep your Ade preferences? [Y/n] "
    read -r response
    case "$response" in
        [nN]|[nN][oO])
            rm -rf "$HOME/.config/ade"
            echo "Preferences removed."
            ;;
        *)
            echo "Preferences kept."
            ;;
    esac
}

main() {
    platform="$(uname -s)"
    channel="${ADE_CHANNEL:-${ZED_CHANNEL:-stable}}"

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    "$platform"

    echo "Ade has been uninstalled"
}

linux() {
    appid="me.farshed.Ade"
    case "$channel" in
      dev) db_suffix="dev" ;;
      *) db_suffix="stable" ;;
    esac

    # Remove the app directory
    rm -rf "$HOME/.local/ade.app"

    # Remove the binary symlink
    rm -f "$HOME/.local/bin/ade"

    # Remove the .desktop file
    rm -f "$HOME/.local/share/applications/${appid}.desktop"

    # Remove the database directory for this channel
    rm -rf "$HOME/.local/share/ade/db/0-$db_suffix"

    # Remove socket file
    rm -f "$HOME/.local/share/ade/ade-$db_suffix.sock"

    # Remove the entire Ade directory if no installations remain
    if check_remaining_installations; then
        rm -rf "$HOME/.local/share/ade"
        prompt_remove_preferences
    fi

    rm -rf "$HOME/.ade_server"
}

macos() {
    app="Ade.app"
    db_suffix="stable"
    app_id="me.farshed.Ade"
    case "$channel" in
      dev)
        db_suffix="dev"
        ;;
    esac

    # Remove the app bundle
    if [ -d "/Applications/$app" ]; then
        rm -rf "/Applications/$app"
    fi

    # Remove the binary symlink
    rm -f "$HOME/.local/bin/ade"

    # Remove the database directory for this channel
    rm -rf "$HOME/Library/Application Support/Ade/db/0-$db_suffix"

    # Remove app-specific files and directories
    rm -rf "$HOME/Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments/$app_id.sfl"*
    rm -rf "$HOME/Library/Caches/$app_id"
    rm -rf "$HOME/Library/HTTPStorages/$app_id"
    rm -rf "$HOME/Library/Preferences/$app_id.plist"
    rm -rf "$HOME/Library/Saved Application State/$app_id.savedState"

    # Remove the entire Ade directory if no installations remain
    if check_remaining_installations; then
        rm -rf "$HOME/Library/Application Support/Ade"
        rm -rf "$HOME/Library/Logs/Ade"

        prompt_remove_preferences
    fi

    rm -rf "$HOME/.ade_server"
}

main "$@"
