
# Inserted by Distrod. Distrod creates a script to load additional WSL session
# environment variables at runtime. You may have to update your ~/.bash_profile
# or ~/.zsh_profile so that it sources this ~/.profile depending on your distro.
if [ -e "{{WSL_ENV_INIT_SH_PATH}}" ]; then
    . "{{WSL_ENV_INIT_SH_PATH}}"
fi
