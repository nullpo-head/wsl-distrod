WINDOWS_DISTROD_PROJECT_PATH := distrod
OUTPUT_ROOTFS_PATH := $(WINDOWS_DISTROD_PROJECT_PATH)/distrod_wsl_launcher/resources/distrod_root.tar.gz

build: distrod-release

rootfs:
	./distrod_packer/distrod_packer ./distrod "$(OUTPUT_ROOTFS_PATH)"

distrod-release:
	./distrod_packer/distrod_packer -r ./distrod opt_distrod.tar.gz

.PHONY: build rootfs distrod-release 
