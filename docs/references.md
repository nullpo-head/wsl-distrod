# References

## Troubleshoot WSL Network Down

Distrod should modify systemd-based systems so that it doesn't break the WSL network,
but possibly you may encounter the situation where Distrod cannot connect to the Internet,
or perhaps your entire WSL network go down.
Don't be panicked. It can be fixed easily :)

### Why did the entire WSL network go down, not just Distrod's?

Sometimes your entire WSL network may go down, not just Distrod's.
This is because in WSL2, all distros share the same network interfaces.
Actually, each distro of WSL2 is not running on an isolated VM, but on a container that WSL's `/init` creates.
And they share the same network namespaces, thus the entire network is down if some distro breaks its network.

### Solution 1: Use Tested Distro with Distrod

1. Fix the network first.

   Broken network affects other distros as well, so let's anyway fix it first.
   Just restarting your WSL2 solves the problem.

   Run

   ```console
   > wsl --shutdown
   ```

   and restart it by starting your NON-Distrod distro. If you start Distrod, it will break the network again.

2. Un-install Distrod

   ```console
   > wsl --unregister Distrod
   ```

3. Install a distro which [README.md](../README.md) says is continuously tested.

   Follow the instruction on README.md. The tested distros will not break the network.

### Solution 2: Debug What Systemd Service is Breaking your Network

If you're familiar with Linux system and Systemd, you will be able to find which systemd service is
breaking the WSL network, and disable it by `# systemctl disable SERVICE_NAME`.

1. Find the service which broke the network.

   Basically, when a WSL2 distro starts,
   WSL will initialize `eth0` and assign an IP address and the default gateway to it.
   Then, WSL also updates `/etc/resolv.conf` with its own DNS server.
   On the other hand, WSL doesn't provide any DHCP server.

   So, maybe some systemd unit broke

   1. `eth0`'s IP / the default gateway by DHCP or
   2. `/etc/resolv.conf`

   Find that from the log from `journalctl`

   ```bash
   sudo journalctl -b
   ```

2. Disable the service

   ```bash
   sudo systemctl disable SERVICE
   ```

3. Restart WSL

   ```console
   > wsl --shutdown
   ```

4. Report the issue to Distrod! :)

### Solution 3: Disable Distrod

1. Disable Distrod

   See [Disable Systemd / Distrod](#disable-systemd--distrod)

2. Restart WSL

   ```console
   > wsl --shutdown
   ```

Even after disabling Distrod, you can continue to use your distro as a regular WSL2 distro.

## Launch WSL 2 on Windows Startup

Please run `enable` command with `--start-on-windows-boot` option.
It's ok to run `enable` command multiple times, even if you have already enabled distrod.

```bash
sudo /opt/distrod/bin/distrod enable --start-on-windows-boot
```

This command registers a Windows task that launches a Distrod distro in WSL2 on Windows startup.
Although the UAC dialog appears when you run `enable --start-on-windows-boot`,
the registered Windows task will run without Windows admin privilege.

Distrod will start automatically when Windows starts, regardless of whether you are logged in or not.

**NOTE**: Distrod runs on Windows startup with a 30 second delay.
You can check if the auto-start succeeded by Windows' Task Scheduler.

See also:

- [Enable Debug Logging of Distrod](#enable-debug-logging-of-distrod)

## Stop Launching WSL 2 on Windows Startup

Open Windows "Task Scheduler" app, and remove the task named as `StartDistrod_YOUR_DISTRO_NAME_for_YOUR_USER_NAME`.

## Forward Ports to outside of Windows

As of October 2021, WSL 2 doesn't forward ports to outside of Windows.
This has plagued many users who want to expose their ssh servers to the outside of Windows,
and there was no easy solution to this problem.

Distrod provides a very simple port-forwarding solution, using systemd.
Distrod has the built-in `portproxy.service`. Enable it by `systemctl`.

**NOTE**: On Windows 11, exe files on WSL FS doesn't work on Windows startup. See [Known bus](#known-bugs).

1. Configure the port numbers to forward

   Write the port numbers in `/opt/distrod/conf/tcp4_ports`, separated by spaces.

   ```console
   $ cat /opt/distrod/conf/tcp4_ports
   0
   $ echo 22 80 443 | sudo tee /opt/distrod/conf/tcp4_ports
   $ cat /opt/distrod/conf/tcp4_ports
   22 80 443
   ```

2. Enable and start `portproxy.service`

   ```console
   $ sudo systemctl enable --now portproxy.service
   Created symlink /etc/systemd/system/multi-user.target.wants/portproxy.service → /run/systemd/system/portproxy.service
   $ sudo systemctl status portproxy.service
   ● portproxy.service - Distrod port exposure service
     Loaded: loaded (/run/systemd/system/portproxy.service; enabled; vendor preset: disabled)
     Active: active (running) since Sat 2021-10-30 21:55:13 JST; 2s ago
   Main PID: 271 (portproxy.exe)
      Tasks: 1 (limit: 61620)
     Memory: 2.8M
     CGroup: /system.slice/portproxy.service
             └─271 /tools/init /opt/distrod/bin/portproxy.exe proxy 172.29.231.165 -t 22 80 443

   Oct 30 21:55:13 machine systemd[1]: Started Distrod port exposure service.
   Oct 30 21:55:13 machine sh[271]: Forwarding 0.0.0.0:22 to 172.29.231.165:22
   Oct 30 21:55:13 machine sh[271]: Forwarding 0.0.0.0:443 to 172.29.231.165:443
   Oct 30 21:55:13 machine sh[271]: Forwarding 0.0.0.0:80 to 172.29.231.165:80
   ```

   Now you should be able to access your services from outside of Windows.

## Install and Run Multiple Distros at the same time

You can install multiple distros by `distrod_wsl_launcher.exe`.
Please run it with `-d new_distro_name` option.

```console
> distrod_wsl_launcher -d new_distrod
```

## Disable Systemd / Distrod

By disabling Distrod, systemd will not run anymore.
You can continue to use your WSL instance as a regular WSL distro.

```bash
sudo /opt/distrod/bin/distrod disable
```

If you also want to completely remove distrod, just delete `/opt/distrod`.

## Open a Shell Session outside the Container for Systemd

Basically, Distrod works by

1. Register Distrod as the login shell in Linux
2. When Distrod is launched by WSL's init as a login shell,
   1. Distrod starts systemd in a simple container
   2. Launch your actual shell within that container

You can escape from this container for debug or other purposes.
Usually, `wsl` command runs every command given to it via the default shell,
that is, Distrod in this case. However, with `-e` option, it runs a command
without a shell. So, launch the shell by the following command to escape from
the Distrod's container.

```console
> wsl -d Distrod -e /bin/bash
```

## Run Distrod as a Standalone One-shot Command

In this usage, distrod works just like genie or subsystemd.

Before starting it, enabling Distrod once is recommended.
Otherwise, it's likely that the network interfaces of the whole WSL system will get broken
by the default systemd network configuration, which is often incompatible with WSL.

```bash
sudo /opt/distrod/bin/distrod enable
sudo /opt/distrod/bin/distrod disable
```

Then, you can start a systemd session by the following command.  
**WARNING:** systemd will clean up files under `/tmp`

```bash
sudo /opt/distrod/bin/distrod start
```

You can get inside the container by

```bash
sudo /opt/distrod/bin/distrod exec -u $(whoami) -- /bin/bash
```

## Enable Debug Logging of Distrod

Edit the Distrod's configuration file and set the debug level.

Add the following lines to `/opt/distrod/conf/distrod.toml`.

```toml
log_level = "trace"
```

With this setting, you will see debug messages in a terminal from Distrod when you open a WSL session.
The message displayed will be different when you start a WSL session for the first time after Windows starts
and when you start a WSL session after that.

In some cases, such as when Distrod starts automatically when Windows starts, it may be difficult to see the messages in the terminal. You can enable logging to `/dev/kmsg`.

```toml
kmsg_log_level = "trace"
```

You can see the log by the following command.

```bash
sudo journalctl -b -k -t Distrod
```

If your system is not running systemd for some reason, then you can check the log by

```bash
sudo grep 'Distrod:' /dev/kmsg
```

## Know Bugs

- Starting the port forwarding service on Windows startup doesn't work on Windows 11,
  because Windows 11 has a bug that it cannot run exe files on WSL's FS on Windows startup.
  Workaround will be implemented soon.
