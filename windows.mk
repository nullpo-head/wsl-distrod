ROOTFS_PATH ?= distrod/distrod_wsl_launcher/resources/distrod_root.tar.gz
OUTPUT_PORT_PROXY_EXE_PATH ?= distrod/target/release/portproxy.exe

build: distro_launcher/x64/distrod_wsl_launcher.exe

distro_launcher: distro_launcher/x64/Release/DistroLauncher-Appx/DistroLauncher-Appx_1.0.0.0_x64.appx

distro_launcher/x64/Release/DistroLauncher-Appx/DistroLauncher-Appx_1.0.0.0_x64.appx: distro_launcher/x64/distrod_wsl_launcher.exe
	cd distro_launcher; cmd.exe /C "build.bat rel"

distro_launcher/x64/distrod_wsl_launcher.exe: distrod_wsl_launcher

distrod/distrod_wsl_launcher/resources/distrod_root.tar.gz: $(ROOTFS_PATH)
	if [ "$$(realpath "$(ROOTFS_PATH)" )" != "$$(realpath distrod/distrod_wsl_launcher/resources/distrod_root.tar.gz)" ]; then \
		cp $(ROOTFS_PATH) $@; \
	fi

distrod_wsl_launcher: distrod/distrod_wsl_launcher/resources/distrod_root.tar.gz
	cd distrod; cargo.exe build --release -p distrod_wsl_launcher

distrod/target/release/portproxy.exe: portproxy.exe
portproxy.exe:
	cd distrod; cargo.exe build --release -p portproxy
	if [ "$$(realpath "$(OUTPUT_PORT_PROXY_EXE_PATH)" )" != "$$(realpath ./distrod/target/release/portproxy.exe)" ]; then \
		cp target/release/port_proxy.exe $(OUTPUT_PORT_PROXY_EXE_PATH); \
	fi

test-win: distrod/distrod_wsl_launcher/resources/distrod_root.tar.gz
	cd distrod; cargo test --verbose -p libs -p portproxy -p distrod_wsl_launcher

.PHONY: build distro_launcher distrod_wsl_launcher portproxy.exe test
