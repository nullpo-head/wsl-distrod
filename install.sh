#!/bin/sh

set -e

HELP_STR="Usage: $0 <command>

command:
    - install
    - update
    - uninstall"

help () {
    echo "$HELP_STR"
}

error_help () {
    error "$HELP_STR"
}

install () {
    mkdir /opt/distrod || error "Failed to create /opt/distrod."
    cd /opt/distrod || error "Could not change directory to /opt/distrod"
    curl -O "$(get_latest_release_url)"
    tar xvf opt_distrod.tar.gz
    rm opt_distrod.tar.gz
    echo "Installation is complete!"
}

uninstall () {
    if grep -o systemd /proc/1/status > /dev/null; then
        error "This uninstall command cannot run inside a running Distrod distro."
        error "To uninstall it, do the following first."
        error "1. /opt/distrod/distrod disable  # Stop systemd from starting as init"
        error "2. wsl.exe --shutdown  # Terminate WSL2"
        error "After that, Systemd will not run as the init and you can run uninstall."
        exit 1
    fi
    rm -rf /opt/distrod
    echo "Distrod has been uninstalled!"
}

update () {
    cd /opt/distrod || error "Could not change directory to /opt/distrod"
    curl -O "$(get_latest_release_url)"
    EXCLUDE=""
    for FILE in /opt/distrod/conf/*; do
        FILE=${FILE#/opt/distrod/}
        if printf "%s" "$FILE" | grep -E ' '; then
            error "Found a file with a name containing spaces. Please remove it. Aborting update command."
            exit 1
        fi
        EXCLUDE="$EXCLUDE --exclude $FILE"
    done
    # shellcheck disable=SC2086
    tar xvf opt_distrod.tar.gz $EXCLUDE
    echo "Distrod has been updated!"
}


REPO="nullpo-head/wsl-distrod"
LATEST_RELEASE_URL=""

get_latest_release_url () {
    if [ -n "$LATEST_RELEASE_URL" ]; then
        echo "$LATEST_RELEASE_URL"
    fi
    TAG=$(curl --silent "https://api.github.com/repos/$REPO/releases/latest" | grep tag_name | grep -Eo 'v[0-9.]*')
    LATEST_RELEASE_URL="https://github.com/$REPO/releases/download/$TAG/opt_distrod.tar.gz"
    printf "%s" "$LATEST_RELEASE_URL"
}

error () {
    echo "$@" >&2
}


if [ -z "$1" ]; then
    error_help
    exit 1
fi

if [ "$(whoami)" != "root" ]; then
    error "You must be root to run this script, please use sudo ./install.sh"
    exit 1
fi

case "$1" in
-h|--help)
    echo "$HELP_STR"
    exit 0
    ;;
install)
    install
    exit 0
    ;;
uninstall)
    uninstall
    exit 0
    ;;
update)
    update
    exit 0
    ;;
-*)
    error "Error: Unknown flag $1"
    exit 1
    ;;
*) # preserve positional arguments
    error "Error: Unknown command $1"
    exit 1
    ;;
esac
