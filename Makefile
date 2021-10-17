WINDOWS_DISTROD_PROJECT_PATH ?= distrod
OUTPUT_ROOTFS_PATH ?= $(WINDOWS_DISTROD_PROJECT_PATH)/distrod_wsl_launcher/resources/distrod_root.tar.gz

build: distrod-release

rootfs:
	./distrod_packer/distrod_packer ./distrod $(OUTPUT_ROOTFS_PATH)

distrod-release: distrod-bins distrod/target/release/portproxy.exe
	./distrod_packer/distrod_packer ./distrod opt_distrod.tar.gz --pack-distrod-opt-dir

distrod-bins:
	cd distrod; cargo build --release -p distrod -p distrod-exec -p portproxy

unit-test-linux:
	cd distrod; cargo test --verbose -p libs -p portproxy -p distrod-exec

integration-test-linux:
	cd distrod/distrod/tests; ./test_runner.sh run

test-linux: lint unittest integtest

lint:
	shellcheck install.sh

ifneq ($(shell uname -a | grep microsoft),)  # This is a WSL environment, which means you can run .exe
ROOTFS_PATH = $(OUTPUT_ROOTFS_PATH)
OUTPUT_PORT_PROXY_EXE_PATH = distrod/target/release/portproxy.exe
distrod_wsl_launcher: distrod-release
include windows.mk
endif

.PHONY: build rootfs distrod-release distrod-bins lint unit-test-linux integration-test-linux test-linux
