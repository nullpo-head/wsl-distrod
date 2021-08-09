WINDOWS_DISTROD_PROJECT_PATH ?= distrod
PORT_PROXY_EXE_PATH ?= $(WINDOWS_DISTROD_PROJECT_PATH)/target/release/portproxy.exe

build: distrod-release

distrod-release: distrod-bins $(PORT_PROXY_EXE_PATH)
	./distrod_packer/distrod_packer -r ./distrod opt_distrod.tar.gz

distrod-bins:
	cd distrod; cargo build --release -p distrod -p distrod-exec -p portproxy

test-linux: lint
	cd distrod; cargo test --verbose -p libs -p portproxy -p distrod-exec -p distrod

lint:
	shellcheck install.sh

ifneq ($(shell uname -a | grep microsoft),)  # This is a WSL environment, which means you can run .exe
ROOTFS_PATH = opt_distrod.tar.gz
distrod_wsl_launcher: distrod-release
include windows.mk
endif

.PHONY: build rootfs distrod-release distrod-bins lint test
