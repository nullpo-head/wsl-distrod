#!/bin/sh

set -e

LATEST_RELEASE_URL="https://github.com/nullpo-head/wsl-distrod/releases/latest/download/opt_distrod.tar.gz"

help () {
    cat <<-eof
Usage: $0 [flags] <command>

flags:
    -r, --release-file <filename>    
                      Use <filename> as opt_distrod.tar.gz instead of downloading from:
                      $LATEST_RELEASE_URL
    -h, --help
                      Displays this help, the same as the help command

command:
    - install
    - update
    - uninstall
    - help (this)
eof
}

error_help () {
  error "$(help)"
}

install () {
    mkdir /opt/distrod || error "Failed to create /opt/distrod."
    cd /opt/distrod || error "Could not change directory to /opt/distrod"
    get_release_file
    tar xvf opt_distrod.tar.gz
    rm opt_distrod.tar.gz
    echo "Installation is complete!"
}

uninstall () {
    if grep -o systemd /proc/1/status > /dev/null; then
        error "This uninstall command cannot run inside a running Distrod distro."
        error "To uninstall it, do the following first."
        error "1. /opt/distrod/bin/distrod disable  # Stop systemd from starting as init"
        error "2. wsl.exe --shutdown  # Terminate WSL2"
        error "After that, Systemd will not run as the init and you can run uninstall."
        exit 1
    fi
    rm -rf /opt/distrod
    echo "Distrod has been uninstalled!"
}

update () {
    cd /opt/distrod || error "Could not change directory to /opt/distrod"
    get_release_file
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
    echo "Ruuning post-update actions..."
    POST_UPDATE="/opt/distrod/misc/distrod-post-update"
    if [ -e "${POST_UPDATE}" ]; then
        "${POST_UPDATE}"
    fi
    echo "Distrod has been updated!"
}

get_release_file() {
    if [ -n "$RELEASE_FILE" ]; then
        if [ "$(realpath "$RELEASE_FILE")" != "$(realpath opt_distrod.tar.gz)" ]; then
            cp "$RELEASE_FILE" opt_distrod.tar.gz
        fi
    else
        curl -L -O "${LATEST_RELEASE_URL}"
    fi
}

error () {
    echo "$@" >&2
}

if [ -z "$1" ]; then
    help
    error_help
    exit 1
fi

unset NEEDS_ROOT

COMMAND=
while [ -n "$1" ]; do
    case "$1" in
    -h|--help|help)
        COMMAND=help
        break
        ;;
    install)
        COMMAND=install
        NEEDS_ROOT=1
        shift
        ;;
    uninstall)
        COMMAND=uninstall
        NEEDS_ROOT=1
        shift
        ;;
    update)
        COMMAND=update
        NEEDS_ROOT=1
        shift
        ;;
    -r|--release-file)
        RELEASE_FILE="$(realpath "$2")"
        shift 2
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
done

if [ -n "$NEEDS_ROOT" ] && [ "$(whoami)" != "root" ]; then
    printf "You must be root to use the '${COMMAND}' command, please use 'sudo ${0} ${COMMAND}'\n"
    exit 1
fi

"$COMMAND"
