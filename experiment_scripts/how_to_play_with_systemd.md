# How to Play with Systemd

This doc explains how to create an experimental environment on your local machine to play around with Systemd and Linux namespaces.

## 1. Prepare disk images

We will prepare two Linux user-space filesystems as our experimental environment.
These two distros do not necessarily have to be different distros, but it is more convenient if they are.
This is because it makes it easier to know which environment you are in when you enter a new namespace.

There are two options you have.

### 1. Use VMs

**Recommended, but a little time-consuming**

1. Set up two VMs on some virtualization software such as VirtualBox or Hyper-V.

   For example, I have set up Ubuntu and Fedora on Hyper-V. 
  
   Now you should have two VMs and two virtual disks for them.

2. Attach one of your virtual disks to another VM. Attach non-Ubuntu's disk to Ubuntu VM is recommended.

   At least Hyper-V has a feature to attach an existing virtual disk to a VM.
   Please attach a disk of one of your VMs to another by that feature.

3. Launch the Ubuntu VM which the another disk is attached to

   If you're using Ubuntu with GUI installed, Ubuntu will automatically find your another disk.
   Or, 

   ```
   sudo mkdir /mnt/fedora  # or anything
   sudo mount -oro,noload /dev/sdb2 /mnt/fedora  #dev/sdb2 may change accroding to your env
   ```

4. Run sshd

    It's recommended to connect to Ubuntu via ssh for the further work.

### 2. Use WSL 2 

**Easy and the same setting as the final product, but perhaps oversimplifies the problem**

This setting will mess up your WSL environment. You should know how to restart WSL.

In cmd.exe
```shell
wsl --shutdown
```

In this option, you choose your WSL environment as the host distro, and you download some another distro
from the Internet.

1. Download a distro image from [here](https://uk.images.linuxcontainers.org/images/)

2. Uncompress the image to anywhere you want

The potential fault of this preparation is that the downloaded image has never been initialized nor booted as an actual distro.
It might be possible that something important is missing in this setting, such as network initialization.

## 2. Mount the filesystem

Mount the filesystem you prepared.

```shell
mkdir -p ~/systemdexp/{upper,work,root}
sudo mount -t overlay overlay -o lowerdir=/path/to/your/fs/downloaded,upperdir=${HOME}/systemdexp/upper/,workdir=${HOME}/systemdexp/work ${HOME}/systemdexp/root
```

The root file system is now mounted.
These commands have set up overlayfs.
This means that all file writes and changes will be volatile, not written to the actual file system you prepared.
To undo all changes, run

```shell
sudo umount ~/systemdexp/root
rm -f ~/systemdexp
```

and by doing the previous steps again.


## 3. Enter the namespaces and launch systemd

Enter namespaces

```shell
cd ~/systemdexp
sudo unshare -mufp /bin/bash
```

Switch the root filesystem

```shell
mount --bind root root
cd root
mkdir mnt/distrod_root
pivot_root . mnt/distrod_root
mount -t proc none /proc
mount -t tmpfs none /tmp
mount -t devtmpfs none /dev
```

Unmount the host special filesystems for safety

```shell
cd /
# do several times
for m in $(mount | grep -Eo '/mnt/distrod_root/([^ ]*)' | grep -v systemdexp); do
    umount "$m";
done
for m in $(mount | grep -Eo '/mnt/distrod_root/([^ ]*)' | grep -v systemdexp); do
    umount "$m";
done
for m in $(mount | grep -Eo '/mnt/distrod_root/([^ ]*)' | grep -v systemdexp); do
    umount "$m";
done
```

Next, launch systemd. If you're using GUI in VM, it may crash... I'm not sure, because I'm using CUI Ubunutu and logging in by ssh, but at least it seems that the new Systemd grabs /dev/tty[0-9] because `devtmpfs` is not isolated by namespaces.

Anyway, Systemd starts by

```shell
exec /sbin/init --unit=multi-user.target
```

Systemd runs until you kill it.


## 4. Open a Bash session inside the new namespace

Open another terminal.

Find the PID of the systemd inside the namespace

```console
$ pgrep -alf /sbin/init
1 /sbin/init
75155 /sbin/init --unit=multi-user.target
```

The non-pid-1 systemd is the systemd instance you launched.

Start a bash session as its child.

```shell
sudo nsenter -m -n -i -p -t $(pgrep -f "/sbin/init --unit=multi-user.target") machinectl shell .host   # machinectl prepares pty for you
```

Now you have the environment for your experiments!
