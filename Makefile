WINDOWS_DISTROD_PROJECT_PATH := distrod
OUTPUT_ROOTFS_PATH := $(WINDOWS_DISTROD_PROJECT_PATH)/distrod_wsl_launcher/resources/distrod_root.tar.gz
PORT_PROXY_EXE_PATH := $(WINDOWS_DISTROD_PROJECT_PATH)/target/release/portproxy.exe

build: distrod-release

rootfs:
	./distrod_packer/distrod_packer ./distrod "$(OUTPUT_ROOTFS_PATH)"

distrod-release: distrod-bins $(PORT_PROXY_EXE_PATH)
	./distrod_packer/distrod_packer -r ./distrod "$(OUTPUT_ROOTFS_PATH)"

distrod-bins:
	cd distrod; cargo build --release -p distrod -p distrod-exec -p portproxy

lint:
	shellcheck install.sh

ifneq ($(shell uname -a | grep microsoft),)  # This is a WSL environment, which means you can run .exe
distrod_wsl_launcher: distrod-release
include windows.mk
endif

.PHONY: build rootfs distrod-release distrod-bins lint
