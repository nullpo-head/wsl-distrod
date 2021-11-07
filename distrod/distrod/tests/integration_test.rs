use std::{fs::File, io::BufWriter, path::PathBuf, process::Command, time::Duration};

use anyhow::{Context, Result};
use libs::{
    cli_ui::build_progress_bar,
    container_org_image::fetch_container_org_image,
    distro_image::{
        download_file_with_progress, DefaultImageFetcher, DistroImage, DistroImageFetcher,
        DistroImageFile, DistroImageList,
    },
};
use once_cell::sync::Lazy;

static DISTROD_SETUP: Lazy<DistrodSetup> = Lazy::new(|| {
    let distrod_setup = DistrodSetup::new(&TestEnvironment::distro_in_testing());
    distrod_setup.create();
    distrod_setup.start();
    std::thread::sleep(Duration::from_secs(5));
    distrod_setup
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
    let query_systemctl = || -> std::process::Output {
        let mut systemctl = DISTROD_SETUP.new_command();
        systemctl.args(&["exec", "systemctl", "status"]);
        systemctl.output().unwrap()
    };
    for _ in 0..20 {
        std::thread::sleep(Duration::from_secs(6));
        let output = query_systemctl();
        eprintln!(
            "Querying systemctl's status. stdout: '{}', stderr: '{}'",
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .take(4)
                .collect::<Vec<_>>()
                .join("\n"),
            String::from_utf8_lossy(&output.stderr)
        );

        if !String::from_utf8_lossy(&output.stdout).contains("State:") {
            continue;
        }
        if !String::from_utf8_lossy(&output.stdout).contains("State: starting") {
            break;
        }
    }
    // Output debug information for the case that the test fails.
    let output = query_systemctl();
    show_debug_systemd_info();
    assert!(String::from_utf8_lossy(&output.stdout).contains("State: running"));
    // Check that one more time in 1 minute to see if there are any units that have crashed
    std::thread::sleep(Duration::from_secs(60));
    let output = query_systemctl();
    show_debug_systemd_info();
    assert!(String::from_utf8_lossy(&output.stdout).contains("State: running"));
}

fn show_debug_systemd_info() {
    let inner = || -> Result<()> {
        let mut systemctl = DISTROD_SETUP.new_command();
        systemctl.args(&["exec", "systemctl", "status"]);
        let output = systemctl
            .output()
            .with_context(|| "Failed to run systemctl.")?;
        eprintln!(
            "$ systemctl status => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .take(4)
                .collect::<Vec<_>>()
                .join("\n"),
            String::from_utf8_lossy(&output.stderr)
        );

        let mut systemctl = DISTROD_SETUP.new_command();
        systemctl.args(&["exec", "--", "systemctl", "--failed"]);
        let output = systemctl.output().with_context(|| "Failed to run ip.")?;
        eprintln!(
            "$ systemctl --failed => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let mut bash = DISTROD_SETUP.new_command();
        bash.args(&[
            "exec",
            "--",
            "bash",
            "-c",
            "for u in $(systemctl --failed | grep failed | awk '{print $2}'); do journalctl -u \"$u\" | cat; done",
        ]);
        let output = bash.output().with_context(|| "Failed to run ip.")?;
        eprintln!(
            "journalctl => \n{}\n{}",
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
    // Wait for a while because Systemd may break the network only after some delay.
    std::thread::sleep(Duration::from_secs(15));

    // Output debug information for the case that the test fails.
    show_debug_ip_info();

    // Use Python instead of simple ping because ping does not work on GitHub Actions.
    let mut sh = DISTROD_SETUP.new_command();
    sh.args(&["exec", "--", "sh", "-c"]);
    sh.arg(gen_connection_check_shell_script(&format!(
        "http://{}",
        &TestEnvironment::ip_addr_for_connection_test()
    )));
    let child = sh.status().unwrap();
    assert!(child.success());
}

#[test]
fn test_name_can_be_resolved() {
    // Wait for a while because Systemd may break the network only after some delay.
    std::thread::sleep(Duration::from_secs(15));

    // Output debug information for the case that the test fails.
    show_debug_ip_info();

    // Use Python instead of simple ping because ping does not work on GitHub Actions.
    let mut sh = DISTROD_SETUP.new_command();
    sh.args(&["exec", "--", "sh", "-c"]);
    sh.arg(gen_connection_check_shell_script("http://www.example.com"));
    let child = sh.status().unwrap();
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

        let mut ping = DISTROD_SETUP.new_command();
        ping.args(&["exec", "--", "ping", "-c", "1", "192.168.99.1"]); // 192.168.99.1 is the IP of the host ns.
        let output = ip.output().with_context(|| "Failed to run ping.")?;
        eprintln!(
            "$ ping 192.168.99.1 => \n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        Ok(())
    };
    if let Err(e) = inner() {
        eprintln!("{}", e);
    }
}

fn gen_connection_check_shell_script(uri: &str) -> String {
    let mut script = String::from("if false\n then : ;\n");
    // GitHub Action doesn't allow you to use ICMP connection, so you have to
    // test the connection by TCP or UDP.
    let commands = [
        gen_connection_check_curl_command(uri),
        gen_connection_check_python_command(uri),
        gen_connection_check_perl_command(uri),
        gen_connection_check_apt_fallback(uri),
        gen_connection_check_tracepath_command(uri),
        gen_connection_check_zypper_fallback(uri),
    ];
    for (command_name, whole_command) in commands.iter() {
        script.push_str(&format!(
            "elif {} > /dev/null; then\n {}\n",
            &command_name, &whole_command
        ))
    }
    script.push_str("else\n echo no command available >&2; exit 1\n fi");
    eprintln!("{}", script);
    script
}

fn gen_connection_check_tracepath_command(uri: &str) -> (&'static str, String) {
    (
        "command -v tracepath",
        format!(
            r"tracepath -4 -m 10 {}",
            uri.strip_prefix("http://")
                .or_else(|| uri.strip_prefix("https://"))
                .unwrap_or(uri)
        ),
    )
}

fn gen_connection_check_curl_command(uri: &str) -> (&'static str, String) {
    ("command -v curl", format!("curl -s {} > /dev/null", uri))
}

fn gen_connection_check_python_command(uri: &str) -> (&'static str, String) {
    let python_script = format!(
        "import urllib.request\n\
         import sys\n\
         res = urllib.request.urlopen(\"{}\")\n\
         sys.exit(0 if res.read() is not None else 1)",
        uri
    );
    (
        "command -v python3",
        format!("python3 -c '{}'", &python_script),
    )
}

fn gen_connection_check_perl_command(uri: &str) -> (&'static str, String) {
    (
        "perl -e 'use LWP::Simple' 2> /dev/null",
        format!(
            r#"perl -e 'use LWP::Simple; $cont = get("{}"); die "" if (! defined $cont);'"#,
            uri
        ),
    )
}

/// No other command is available, so fallback to apt, though doesn't check connection without name resolving.
fn gen_connection_check_apt_fallback(uri: &str) -> (&'static str, String) {
    if ('0'..='9').contains(&uri.chars().last().unwrap()) {
        // this is an ip address
        ("command -v apt", "true".to_owned())
    } else {
        ("command -v apt", "sudo apt update".to_owned())
    }
}

/// No other command is available, so fallback to zypper, though doesn't check connection without name resolving.
fn gen_connection_check_zypper_fallback(uri: &str) -> (&'static str, String) {
    if ('0'..='9').contains(&uri.chars().last().unwrap()) {
        // this is an ip address
        ("command -v zypper", "true".to_owned())
    } else {
        ("command -v zypper", "sudo zypper refresh".to_owned())
    }
}

#[test]
fn test_wslg_socket_is_available() {
    // Wait for a while until Systemd initializes /tmp
    std::thread::sleep(Duration::from_secs(15));

    let mut test = DISTROD_SETUP.new_command();
    test.args(&["exec", "--", "test", "-e", "/run/tmpfiles.d/x11.conf"]);
    let child = test.status().unwrap();
    assert!(child.success());

    let mut ls = DISTROD_SETUP.new_command();
    ls.args(&["exec", "--", "ls", "-ld", "/tmp/.X11-unix"]);
    let output = ls.output().unwrap();
    let output = String::from_utf8_lossy(&output.stdout);
    eprintln!("output of `ls -ld /tmp/.X11-unix`: {}", output);
    assert!(output.ends_with("-> /mnt/wslg/.X11-unix\n"));
}

#[test]
fn test_profile_initializes_additional_wsl_envs() {
    let mut echo_distro_name = DISTROD_SETUP.new_command();
    echo_distro_name.env_clear();
    echo_distro_name.env("WSL_DISTRO_NAME", "");
    echo_distro_name.args(&[
        "exec",
        "--",
        "bash",
        "-c",
        "echo ETC_ENVIRONMENT=loaded >> /etc/environment; echo $WSL_DISTRO_NAME",
    ]);
    let output = echo_distro_name.output().unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    let output = String::from_utf8_lossy(&output.stdout);
    assert_eq!("", output.trim());

    let mut echo_distro_name = DISTROD_SETUP.new_command();
    echo_distro_name.env_clear();
    echo_distro_name.env("WSL_DISTRO_NAME", "");
    echo_distro_name.args(&[
        "exec",
        "--",
        "bash",
        "--login",
        "-c",
        // make sure WSL_DISTRO_NAME is initialized by profile.d, not by /etc/environment
        "echo -ne $ETC_ENVIRONMENT; echo $WSL_DISTRO_NAME",
    ]);
    let output = echo_distro_name.output().unwrap();
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    let output = String::from_utf8_lossy(&output.stdout);
    assert_eq!("DUMMY_DISTRO", output.trim());

    let mut create_user = DISTROD_SETUP.new_command();
    create_user.args(&[
        "exec",
        "--",
        "useradd",
        "-m",
        "test_user",
        "--shell",
        "/bin/bash",
    ]);
    let child = create_user.status().unwrap();
    assert!(child.success());
    let mut cat_profile = DISTROD_SETUP.new_command();
    cat_profile.env_clear();
    cat_profile.env("WSL_DISTRO_NAME", "");
    cat_profile.args(&[
        "exec",
        "--user",
        "test_user",
        "--",
        "bash",
        "--login",
        "-c",
        // make sure WSL_DISTRO_NAME is initialized by profile.d, not by /etc/environment
        "echo -ne $ETC_ENVIRONMENT; echo $WSL_DISTRO_NAME",
    ]);
    let output = cat_profile.output().unwrap();
    let output = String::from_utf8_lossy(&output.stdout);
    assert_eq!("DUMMY_DISTRO", output.trim());
}

#[test]
fn test_distrod_is_in_path() {
    let mut echo_distrod_path = DISTROD_SETUP.new_command();
    echo_distrod_path.env_clear();
    echo_distrod_path.args(&["exec", "--", "bash", "--login", "-c", "echo $PATH"]);
    let output = echo_distrod_path.output().unwrap();
    eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    let output = String::from_utf8_lossy(&output.stdout);
    eprintln!("stdout: {}", output);
    assert!(output.contains("/opt/distrod/bin"));
}

#[tokio::test]
async fn test_distro_download_url_is_live() {
    let distro_image =
        fetch_container_org_image_by_distro_name(TestEnvironment::distro_in_testing()).await;
    assert!(distro_image.is_ok());
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
            bin_path: TestEnvironment::distrod_bin_path(),
            install_dir: TestEnvironment::install_dir(),
        }
    }

    fn create(&self) {
        let image = setup_distro_image(&self.name);
        let mut distrod = self.new_command();
        distrod.args(&[
            "-l",
            "trace",
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
            "-l",
            "trace",
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

#[tokio::main]
async fn setup_distro_image(distro_name: &str) -> PathBuf {
    let local_cache_path =
        TestEnvironment::image_cache_dir().join(&format!("{}/{}.tar.xz", distro_name, distro_name));
    if local_cache_path.exists() {
        return local_cache_path;
    }

    let local_cache_dir = local_cache_path.parent().unwrap();
    if !local_cache_dir.exists() {
        std::fs::create_dir_all(&local_cache_dir).unwrap();
    }
    let local_cache = File::create(&local_cache_path).unwrap();
    let mut tar_xz = BufWriter::new(local_cache);

    let distro_image = fetch_container_org_image_by_distro_name(distro_name.to_owned())
        .await
        .unwrap();
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

async fn fetch_container_org_image_by_distro_name(distro_name: String) -> Result<DistroImage> {
    let choose_container_org_image_by_distro_name =
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
    fetch_container_org_image(&choose_container_org_image_by_distro_name).await
}

struct TestEnvironment;

impl TestEnvironment {
    pub fn distrod_bin_path() -> PathBuf {
        let mut pathbuf = std::env::current_exe().unwrap();
        pathbuf.pop();
        // https://github.com/rust-lang/cargo/issues/5758
        if pathbuf.ends_with("deps") {
            pathbuf.pop();
        }
        pathbuf.push("distrod");
        pathbuf
    }

    pub fn install_dir() -> PathBuf {
        PathBuf::from(TestEnvironment::get_var("DISTROD_INSTALL_DIR"))
    }

    pub fn image_cache_dir() -> PathBuf {
        PathBuf::from(TestEnvironment::get_var("DISTROD_IMAGE_CACHE_DIR"))
    }

    pub fn ip_addr_for_connection_test() -> String {
        TestEnvironment::get_var("RELIABLE_CONNECTION_IP_ADDRESS")
    }

    pub fn distro_in_testing() -> String {
        TestEnvironment::get_var("DISTRO_TO_TEST")
    }

    fn get_var(var_name: &str) -> String {
        let env_by_testwrapper = std::env::var(var_name);
        if env_by_testwrapper.is_err() {
            panic!(
                "The test wapper script should set {} environment variable.",
                var_name
            );
        }
        env_by_testwrapper.unwrap()
    }
}
