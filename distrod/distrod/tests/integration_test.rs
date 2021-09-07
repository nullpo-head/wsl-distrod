use std::{fs::File, io::BufWriter, path::PathBuf, process::Command, time::Duration};

use anyhow::{Context, Result};
use libs::{
    cli_ui::build_progress_bar,
    distro_image::{
        download_file_with_progress, DefaultImageFetcher, DistroImage, DistroImageFetcher,
        DistroImageFile, DistroImageList,
    },
    lxd_image::fetch_lxd_image,
};
use once_cell::sync::Lazy;

static DISTROD_SETUP: Lazy<DistrodSetup> = Lazy::new(|| {
    let distrod_install_info = DistrodSetup::new("ubuntu");
    distrod_install_info.create();
    distrod_install_info.start();
    std::thread::sleep(Duration::from_secs(5));
    distrod_install_info
});

#[test]
fn test_exec_cmd() {
    let mut echo = DISTROD_SETUP.new_command();
    echo.args(&["exec", "echo", "foo"]);
    let output = echo.output().unwrap();
    assert_eq!("foo\n", String::from_utf8_lossy(&output.stdout));
}

#[test]
fn test_init_is_sytemd() {
    let mut cat = DISTROD_SETUP.new_command();
    cat.args(&["exec", "cat", "/proc/1/stat"]);
    let output = cat.output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("(systemd)"));
}

#[test]
fn test_no_systemd_unit_is_failing() {
    let mut output = None;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(3));
        let mut systemctl = DISTROD_SETUP.new_command();
        systemctl.args(&["exec", "systemctl", "status"]);
        output = Some(systemctl.output().unwrap());

        let o = &output.as_ref().unwrap();
        eprintln!(
            "Querying systemctl's status. stdout: '{}', stderr: '{}'",
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(4)
                .collect::<Vec<_>>()
                .join("\n"),
            String::from_utf8_lossy(&o.stderr)
        );

        if !String::from_utf8_lossy(&output.as_ref().unwrap().stdout).contains("State:") {
            continue;
        }
        if !String::from_utf8_lossy(&output.as_ref().unwrap().stdout).contains("State: starting") {
            break;
        }
    }
    // Output debug information for the case that the test fails.
    show_debug_systemd_info();
    assert!(String::from_utf8_lossy(&output.unwrap().stdout).contains("State: running"));
}

fn show_debug_systemd_info() {
    let inner = || -> Result<()> {
        let mut ip = DISTROD_SETUP.new_command();
        ip.args(&["exec", "systemctl", "status"]);
        let output = ip.output().with_context(|| "Failed to run systemctl.")?;
        eprintln!(
            "$ systemctl status => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .take(4)
                .collect::<Vec<_>>()
                .join("\n"),
            String::from_utf8_lossy(&output.stderr)
        );

        let mut ip = DISTROD_SETUP.new_command();
        ip.args(&["exec", "--", "systemctl", "--failed"]);
        let output = ip.output().with_context(|| "Failed to run ip.")?;
        eprintln!(
            "$ systemctl --failed => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        Ok(())
    };
    if let Err(e) = inner() {
        eprintln!("{}", e);
    }
}

#[test]
fn test_systemd_service_has_wsl_envs() {
    let mut output = None;
    for _ in 0..5 {
        let mut cat_env = DISTROD_SETUP.new_command();
        cat_env.args(&["exec", "--", "bash", "-c"]);
        cat_env.arg(
            r#"
            for p in /proc/[0-9]*; do
                # check if the parent is the init process (PID 1)
                if grep -E 'PPid:[^0-9]*1[^0-9]*' "$p/status"; then
                    cat "$p/environ"
                fi
            done"#,
        );
        output = Some(cat_env.output().unwrap());
        let o = &output.as_ref().unwrap();
        eprintln!(
            "Debug: cat_env. stdout: '{}', stderr: '{}'",
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(4)
                .collect::<Vec<_>>()
                .join("\n"),
            String::from_utf8_lossy(&o.stderr)
        );
        if String::from_utf8_lossy(&output.as_ref().unwrap().stdout)
            .trim_start()
            .is_empty()
        {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        break;
    }
    assert!(String::from_utf8_lossy(&output.unwrap().stdout).contains("WSL_INTEROP"));
}

#[test]
fn test_sudo_initializes_wsl_envs() {
    let mut sudo_env = DISTROD_SETUP.new_command();
    sudo_env.args(&["exec", "--", "sudo", "env"]);
    let output = sudo_env.output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("WSL_INTEROP"));
}

#[test]
fn test_global_ip_is_reachable() {
    // Skip for now until we change the image from Canonical's to LXD's.
    std::thread::sleep(Duration::from_secs(15));

    // Output debug information for the case that the test fails.
    show_debug_ip_info();

    let mut ping = DISTROD_SETUP.new_command();
    ping.args(&["exec", "--", "ping", "-c", "10", "8.8.8.8"]);
    let child = ping.status().unwrap();
    assert!(child.success());
}

#[test]
fn test_name_can_be_resolved() {
    // Wait for a while because Systemd may break the network only after some delay.
    std::thread::sleep(Duration::from_secs(15));

    // Output debug information for the case that the test fails.
    show_debug_ip_info();

    let mut ping = DISTROD_SETUP.new_command();
    ping.args(&["exec", "--", "ping", "-c", "10", "www.google.com"]);
    let child = ping.status().unwrap();
    assert!(child.success());
}

fn show_debug_ip_info() {
    let inner = || -> Result<()> {
        let mut ip = DISTROD_SETUP.new_command();
        ip.args(&["exec", "ip", "a"]);
        let output = ip.output().with_context(|| "Failed to run ip.")?;
        eprintln!(
            "$ ip a => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let mut ip = DISTROD_SETUP.new_command();
        ip.args(&["exec", "ip", "route", "show"]);
        let output = ip.output().with_context(|| "Failed to run ip.")?;
        eprintln!(
            "$ ip route show => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        Ok(())
    };
    if let Err(e) = inner() {
        eprintln!("{}", e);
    }
}

struct DistrodSetup {
    name: String,
    bin_path: PathBuf,
    install_dir: PathBuf,
}

impl DistrodSetup {
    fn new(name: &str) -> DistrodSetup {
        DistrodSetup {
            name: name.to_owned(),
            bin_path: get_bin_path(),
            install_dir: get_test_install_dir(),
        }
    }

    fn create(&self) {
        let image = setup_distro_image(&self.name);
        let mut distrod = self.new_command();
        distrod.args(&[
            "create",
            "--image-path",
            image.to_str().unwrap(),
            "--install-dir",
            self.install_dir.as_path().to_str().unwrap(),
        ]);
        let exit_status = distrod.status().unwrap();
        assert!(exit_status.success());
    }

    fn start(&self) {
        let mut distrod = self.new_command();
        distrod.args(&[
            "start",
            "--rootfs",
            self.install_dir.as_path().to_str().unwrap(),
        ]);
        let exit_status = distrod.status().unwrap();
        assert!(exit_status.success());
    }

    fn new_command(&self) -> Command {
        let mut distrod = Command::new("sudo");
        distrod.arg("-E");
        distrod.arg(self.bin_path.as_path().as_os_str());
        distrod
    }
}

fn get_bin_path() -> PathBuf {
    let mut pathbuf = std::env::current_exe().unwrap();
    pathbuf.pop();
    // https://github.com/rust-lang/cargo/issues/5758
    if pathbuf.ends_with("deps") {
        pathbuf.pop();
    }
    pathbuf.push("distrod");
    pathbuf
}

fn get_test_install_dir() -> PathBuf {
    let env_by_testwrapper = std::env::var("DISTROD_INSTALL_DIR");
    if env_by_testwrapper.is_err() {
        panic!("The test wapper script should set DISTROD_INSTALL_DIR environment variable.");
    }
    PathBuf::from(env_by_testwrapper.unwrap())
}

#[tokio::main]
async fn setup_distro_image(distro_name: &str) -> PathBuf {
    let local_cache_path = PathBuf::from(format!(
        "{}/{}/rootfs.tar.xz",
        get_image_download_dir(),
        distro_name
    ));
    if local_cache_path.exists() {
        return local_cache_path;
    }

    let local_cache_dir = local_cache_path.parent().unwrap();
    if !local_cache_dir.exists() {
        std::fs::create_dir_all(&local_cache_dir).unwrap();
    }
    let local_cache = File::create(&local_cache_path).unwrap();
    let mut tar_xz = BufWriter::new(local_cache);

    let distro_image = fetch_lxd_image_by_distro_name(distro_name.to_owned()).await;
    match distro_image.image {
        DistroImageFile::Local(_) => {
            panic!("The image file should not be a local file");
        }
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            download_file_with_progress(&url, build_progress_bar, &mut tar_xz)
                .await
                .unwrap();
            log::info!("Download done.");
        }
    }

    local_cache_path
}

fn get_image_download_dir() -> String {
    let env_by_testwrapper = std::env::var("DISTROD_IMAGE_CACHE_DIR");
    if env_by_testwrapper.is_err() {
        panic!("The test wapper script should set DISTROD_IMAGE_CACHE_DIR environment variable.");
    }
    env_by_testwrapper.unwrap()
}

async fn fetch_lxd_image_by_distro_name(distro_name: String) -> DistroImage {
    let choose_lxd_image_by_distro_name =
        move |list: DistroImageList| -> Result<Box<dyn DistroImageFetcher>> {
            match list {
                DistroImageList::Fetcher(_, fetchers, default) => {
                    let distro_by_name = fetchers
                        .iter()
                        .find(|fetcher| fetcher.get_name() == distro_name);
                    if distro_by_name.is_some() {
                        return Ok(fetchers
                            .into_iter()
                            .find(|fetcher| fetcher.get_name() == distro_name)
                            .unwrap());
                    }
                    let default = match default {
                        DefaultImageFetcher::Index(index) => fetchers[index].get_name().to_owned(),
                        DefaultImageFetcher::Name(name) => name,
                    };
                    Ok(fetchers
                        .into_iter()
                        .find(|fetcher| fetcher.get_name() == default)
                        .unwrap())
                }
                DistroImageList::Image(_) => {
                    panic!("unreachable");
                }
            }
        };
    fetch_lxd_image(&choose_lxd_image_by_distro_name)
        .await
        .unwrap()
}
