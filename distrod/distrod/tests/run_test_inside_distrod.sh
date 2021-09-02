#!/bin/sh

set -e

###
# Because Distrod doesn't implement nested distrod instance running, 
# this script runs the integration test in a new mount namespace to 
# avoid the problem caused by nesting
###

main () {
    if [ "$1" != run ]; then
        echo "Usage: $0 run" >&2
        exit 1
    fi

    if [ "$2" != "--unshared" ]; then
        sudo unshare -mf sudo -u "$(whoami)" "$0" run --unshared "$(which cargo)"
        exit $?
    fi

    if [ -z "$3" ]; then
        echo "Error: Internal usage: $0 run --unshared path_to_cargo" >&2
        exit 1
    fi

    CARGO="$3"

    prepare_for_nested_distrod
    simulate_wsl_environment

    # run the tests
    "$CARGO" test --verbose -p distrod
}

prepare_for_nested_distrod() {
    # Enter a new mount namespace for testing.
    # To make distrod think it's not inside another distrod,
    # 1. Delete /var/run/distrod.json without affecting the running distrod by 
    #    mounting overlay
    # 2. Unmount directories under /mnt/distrod_root, which is a condition 
    #    distrod checks
    sudo rm -rf /tmp/distrod_test
    mkdir -p /tmp/distrod_test/var/run/upper /tmp/distrod_test/var/run/work
    sudo mount --bind /var/run /var/run
    sudo mount -t overlay overlay -o lowerdir=/var/run,upperdir=/tmp/distrod_test/var/run/upper,workdir=/tmp/distrod_test/var/run/work /var/run
    sudo rm -f /var/run/distrod.json
    sudo umount /mnt/distrod_root/proc || true  # may not exist
}

simulate_wsl_environment() {
    if is_inside_wsl; then
        return
    fi
    # Simulate WSL environment in non-WSL Linux environment such as in
    # GitHub action.
    sudo touch /init
    sudo mkdir /mnt/wsl
    sudo mkdir /run/WSL

    if [ -z "$WSL_DISTRO_NAME" ]; then
        export WSL_DISTRO_NAME=DUMMY_DISTRO
    fi

    if [ -z "$WSL_INTEROP" ]; then
        export WSL_INTEROP=/run/WSL/1_interop
    fi
}

is_inside_wsl() {
    uname -a | grep microsoft > /dev/null
}

main "$@"
