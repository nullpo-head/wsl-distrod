use std::{
    fs::File,
    io::BufWriter,
    ops::Deref,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use once_cell::sync::Lazy;
use tempfile::{NamedTempFile, TempDir};

static DISTROD_BIN_PATH: Lazy<PathBuf> = Lazy::new(get_bin_path);

#[test]
fn integration_test() {
    let ubuntu_image = setup_ubuntu_image();
    let distrod_install_dir = test_create_cmd(&ubuntu_image);
    let distrod_instance = test_start_cmd(distrod_install_dir);
    test_exec_cmd(&distrod_instance);
}

fn test_create_cmd(image: &ImageFile) -> DistrodInstallDir {
    let install_dir = tempfile::tempdir().unwrap();

    let mut distrod = Command::new("sudo");
    distrod.args(&[
        DISTROD_BIN_PATH.as_path().to_str().unwrap(),
        "create",
        "--image-path",
        image.path().to_str().unwrap(),
        "--install-dir",
        install_dir.path().to_str().unwrap(),
    ]);
    let exit_status = distrod.status().unwrap();
    assert!(exit_status.success());

    DistrodInstallDir {
        temp_dir: install_dir,
    }
}

fn test_start_cmd(install_dir: DistrodInstallDir) -> DistrodInstance {
    let distrod_instance = DistrodInstance::new(install_dir);

    let mut distrod = distrod_instance.new_command();
    distrod.args(&[
        "start",
        "--rootfs",
        distrod_instance.install_dir.path().to_str().unwrap(),
    ]);
    let exit_status = distrod.status().unwrap();
    assert!(exit_status.success());

    distrod_instance
}

fn test_exec_cmd(distrod_instance: &DistrodInstance) {
    let mut echo = distrod_instance.new_command();
    echo.args(&["exec", "echo", "foo"]);
    let output = echo.output().unwrap();
    assert_eq!("foo\n", String::from_utf8_lossy(&output.stdout));
}

struct DistrodInstance {
    pub bin_path: PathBuf,
    pub install_dir: DistrodInstallDir,
}

impl DistrodInstance {
    fn new(install_dir: DistrodInstallDir) -> DistrodInstance {
        DistrodInstance {
            bin_path: get_bin_path(),
            install_dir,
        }
    }

    fn new_command(&self) -> Command {
        let mut distrod = Command::new("sudo");
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

impl Drop for DistrodInstance {
    fn drop(&mut self) {
        let mut distrod = self.new_command();
        distrod.args(&["stop", "-9"]);
        let mut child = distrod.spawn().unwrap();
        child.wait().unwrap();
    }
}

// DistrodInstallDir deletes the internal temp_dir after chown when it's dropped since it's owned by root
struct DistrodInstallDir {
    pub temp_dir: TempDir,
}

impl Deref for DistrodInstallDir {
    type Target = TempDir;

    fn deref(&self) -> &Self::Target {
        &self.temp_dir
    }
}

impl Drop for DistrodInstallDir {
    fn drop(&mut self) {
        let mut rm = Command::new("sudo");
        rm.args(&["sh", "-c"]).arg(format!(
            "rm -rf {}/*",
            self.temp_dir.path().to_str().unwrap()
        ));
        let mut child = rm.spawn().unwrap();
        child.wait().unwrap();
    }
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
