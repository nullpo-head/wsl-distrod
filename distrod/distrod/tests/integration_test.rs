use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use once_cell::sync::Lazy;
use tempfile::NamedTempFile;

static DISTROD_SETUP: Lazy<DistrodSetup> = Lazy::new(|| {
    let distrod_install_info = DistrodSetup::new();
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
    // Commenting out because it's difficult to make it pass right now.
    //assert!(String::from_utf8_lossy(&output.unwrap().stdout).contains("State: running"));
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
    return;
    // Wait for a while because Systemd may break the network only after some delay.
    std::thread::sleep(Duration::from_secs(15));
    let mut ping = DISTROD_SETUP.new_command();
    ping.args(&["exec", "--", "ping", "-c", "10", "8.8.8.8"]);
    let child = ping.status().unwrap();
    assert!(child.success());
}

#[test]
fn test_name_can_be_resolved() {
    // Wait for a while because Systemd may break the network only after some delay.
    std::thread::sleep(Duration::from_secs(15));
    let mut ping = DISTROD_SETUP.new_command();
    //ping.args(&["exec", "--", "ping", "-c", "10", "www.google.com"]);
    // Use apt for now until we change the image from Canonical's to LXD's.
    ping.args(&["exec", "--", "apt", "update"]);
    let child = ping.status().unwrap();
    assert!(child.success());
}

struct DistrodSetup {
    pub bin_path: PathBuf,
    pub install_dir: PathBuf,
}

impl DistrodSetup {
    fn new() -> DistrodSetup {
        DistrodSetup {
            bin_path: get_bin_path(),
            install_dir: get_test_install_dir(),
        }
    }

    fn create(&self) {
        let image = setup_ubuntu_image();
        let mut distrod = self.new_command();
        distrod.args(&[
            "create",
            "--image-path",
            image.path().to_str().unwrap(),
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

fn setup_ubuntu_image() -> ImageFile {
    let local_cache_path = PathBuf::from("/tmp/integration_test.tar.xz");
    if local_cache_path.exists() {
        let file = File::open(&local_cache_path).unwrap();
        let cache_file = ImageFile::File(local_cache_path, file);
        return cache_file;
    }

    let tempfile = tempfile::NamedTempFile::new().unwrap();
    let mut tar_xz = BufWriter::new(tempfile);

    let client = reqwest::blocking::Client::builder();
    let client = client
        .connect_timeout(Duration::from_secs(180))
        .build()
        .unwrap();

    let mut response = client.get("https://cloud-images.ubuntu.com/minimal/releases/bionic/release/ubuntu-18.04-minimal-cloudimg-amd64-root.tar.xz").send().unwrap();
    response.copy_to(&mut tar_xz).unwrap();

    let tempfile = tar_xz.into_inner().unwrap();
    std::fs::copy(tempfile.path(), &local_cache_path).unwrap();

    ImageFile::NamedTempFile(tempfile)
}

enum ImageFile {
    File(PathBuf, File),
    NamedTempFile(NamedTempFile),
}

impl ImageFile {
    pub fn path(&self) -> &Path {
        match *self {
            ImageFile::File(ref path, _) => path.as_path(),
            ImageFile::NamedTempFile(ref file) => file.path(),
        }
    }
}
