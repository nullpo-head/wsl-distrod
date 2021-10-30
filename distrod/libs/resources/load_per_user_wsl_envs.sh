#!/bin/sh

# Load additional WSL session environment variables at runtime by sourcing
# a script Distrod creates at runtime. A Linux user who launches Distrod first
# can manipulte the contents of this script, so the script file is per-user one
# to prevent the user from manipulating other user's environment variables.
if [ -e "{{PER_USER_WSL_ENV_INIT_SCRIPT_PATH}}" ]; then
    . "{{PER_USER_WSL_ENV_INIT_SCRIPT_PATH}}"
fi

# If the creator of the script is root, a non-root user loading it is harmless
if [ "$(id -u)" != 0 ] && [ -e "{{ROOT_WSL_ENV_INIT_SCRIPT_PATH}}" ]; then
    . "{{ROOT_WSL_ENV_INIT_SCRIPT_PATH}}"
fi
