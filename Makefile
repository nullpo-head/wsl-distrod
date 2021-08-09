WINDOWS_DISTROD_PROJECT_PATH := distrod
OUTPUT_ROOTFS_PATH := $(WINDOWS_DISTROD_PROJECT_PATH)/distrod_wsl_launcher/resources/distrod_root.tar.gz

build: distrod-release

rootfs:
	./distrod_packer/distrod_packer ./distrod "$(OUTPUT_ROOTFS_PATH)"

distrod-release: distrod-bins distrod/target/release/portproxy.exe
	./distrod_packer/distrod_packer -r ./distrod opt_distrod.tar.gz

distrod-bins:
	cd distrod; cargo build --release -p distrod -p distrod-exec -p portproxy

lint:
	shellcheck install.sh

.PHONY: build rootfs distrod-release distrod-bins lint
