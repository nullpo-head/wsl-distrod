OUTPUT_ROOTFS_PATH ?= distrod/distrod_wsl_launcher/resources/distrod_root.tar.gz

build: distrod-release

rootfs: distrod-bins distrod/target/release/portproxy.exe
	./distrod_packer/distrod_packer ./distrod $(OUTPUT_ROOTFS_PATH)

distrod-release: distrod-bins distrod/target/release/portproxy.exe
	./distrod_packer/distrod_packer ./distrod opt_distrod.tar.gz --pack-distrod-opt-dir

distrod-bins:
	cd distrod; cargo build --release -p distrod -p distrod-exec -p portproxy

unit-test-linux:
	cd distrod; cargo test --verbose -p libs -p portproxy -p distrod-exec ${TEST_TARGETS}

integration-test-linux:
	cd distrod/distrod/tests; ./test_runner.sh run

enter-integration-test-env:
	@echo Run 'cargo test -p distrod'.
	cd distrod/distrod/tests; ./test_runner.sh enter

ALL_DISTROS_IN_TESTING=ubuntu debian archlinux fedora centos almalinux rockylinux kali mint opensuse amazonlinux oracle gentoo
integration-test-linux-all-distros:
	cd distrod/distrod/tests; \
    for distro in $(ALL_DISTROS_IN_TESTING); do \
		 DISTRO_TO_TEST=$${distro} ./test_runner.sh run; \
	done

test-linux: lint unit-test-linux integration-test-linux

lint:
	shellcheck install.sh

clean:
	cd distrod; cargo clean; cargo.exe clean

## Install locally built opt_distrod.tar.gz distrobution. Use 'update' if this system is already running distrod.
install: build
	sudo ./install.sh install --release-file ./opt_distrod.tar.gz
.PHONY: install

## Update local install with built opt_distrod.tar.gz distrobution. Use 'install' if this system doesn't have distrod installed.
update: build
	sudo ./install.sh update --release-file ./opt_distrod.tar.gz
.PHONY: update

ifneq ($(shell uname -a | grep microsoft),)  # This is a WSL environment, which means you can run .exe
ROOTFS_PATH = $(OUTPUT_ROOTFS_PATH)
OUTPUT_PORT_PROXY_EXE_PATH = distrod/target/release/portproxy.exe

$(ROOTFS_PATH): rootfs
include windows.mk

.PHONY: $(ROOTFS_PATH)
endif

.PHONY: build rootfs distrod-release distrod-bins lint clean\
        unit-test-linux enter-integration-test-linux integration-test-linux integration-test-linux-all-distros test-linux
