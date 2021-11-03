# WSL Distro Launcher Reference Implementation
## Introduction 
This is the C++ reference implementation for a Windows Subsystem for Linux (WSL) distribution installer/launcher application. Every distro package must include a launcher app, which is responsible for completing installation & registration of your distro with WSL, and for launching new distro instances atop WSL.

Once you've built your distro launcher, packaged it along with the required art assets, manifest, and distro.tar.gz, and digitally signed the package, you will be able to sideload your distro on your own machine(s).
  
## Important! 
Before publishing your distro to the Windows Store, you must first reach-out to and get approval from the WSL team: wslpartners@microsoft.com. 

Without testing and approval from the WSL team, distros submitted to the store will be rejected. This process is required in order to ensure the quality and integrity of the WSL distro ecosystem, and to safeguard our users.

## Goals
The goal of this project is to enable:

* Linux distribution owners to package and submit an application that runs on top of WSL to the Microsoft Store
* Developers to create custom Linux distributions that can be sideloaded onto their dev machine

## Contents
This reference launcher provides the following functionality:
(where `launcher.exe` is replaced by the distro-specific name)

* `launcher.exe`
  - Launches the user's default shell in the user's home directory.

* `launcher.exe install [--root]`
  - Install the distribution and do not launch the shell when complete.
    - `--root`: Do not create a user account and leave the default user set to root.

* `launcher.exe run <command line>`
  - Run the provided command line in the current working directory. If no command line is provided, the default shell is launched.
  - Everything after `run` is passed to WslLaunchInteractive.

* `launcher.exe config [setting [value]]`
  - Configure settings for this distribution.
  - Settings:
    - `--default-user <username>`: Sets the default user to <username>. This must be an existing user.

* `launcher.exe help`
  - Print usage information.

## Launcher Outline
This is the basic flow of how the launcher code is set up.

1. If the distribution is not registered with WSL, register it. Registration extracts the tar.gz file that is included in your distribution appx.
2. Once the distro is successfully registered, any other pre-launch setup is performed in `InstallDistribution()`. This is where distro-specific setup can be performed. As an example, the reference implementation creates a user account and sets this user account as the default for the distro.
    - Note: The commands used to query and create user accounts in this reference implementation are Ubuntu-specific; change as necessary to match the needs of your distro.
3. Once the distro is configured, parse any other command-line arguments. The details of these arguments are described above, in the [Introduction](#Introduction).

## Project Structure
The distro launcher is comprised of two Visual Studio projects - `launcher` and `DistroLauncher-Appx`. The `launcher` project builds the actual executable that is run when a user launches the app. The `DistroLauncher-Appx` builds the distro package with all the correctly scaled assets and other dependencies. Code changes will be built in the `launcher` project (under `DistroLauncher/`). Manifest changes are applied in the `DistroLauncher-Appx` project (under `DistroLauncher-Appx/`). 

## Getting Started
1. Generate a test certificate:
    1. In Visual Studio, open `DistroLauncher-Appx/MyDistro.appxmanifest`
    1. Select the Packaging tab
    1. Select "Choose Certificate"
    1. Click the Configure Certificate drop down and select Create test certificate.

2. Edit your distribution-specific information in `DistributionInfo.h` and `DistributionInfo.cpp`. **NOTE: The `DistributionInfo::Name` variable must uniquely identify your distribution and cannot change from one version of your app to the next.**
    > Note: The examples for creating a user account and querying the UID are from an Ubuntu-based system, and may need to be modified to work appropriately on your distribution.

3.  Add an icon (.ico) and logo (.png) to the `/images` directory. The logo will be used in the Start Menu and the taskbar for your launcher, and the icon will appear on the Console window.
    > Note: The icon must be named `icon.ico`.

4. Pick the name you'd like to make this distro callable from the command line. For the rest of the README, I'll be using `mydistro` or `mydistro.exe`. **This is the name of your executable** and should be unique.

5. Make sure to change the name of the project in the `DistroLauncher-Appx/DistroLauncher-Appx.vcxproj` file to the name of your executable we picked in step 4. By default, the lines should look like:

``` xml
<PropertyGroup Label="Globals">
  ...
  <TargetName>mydistro</TargetName>
</PropertyGroup>
```

So, if I wanted to instead call my distro "TheBestDistroEver", I'd change this to:
``` xml
<PropertyGroup Label="Globals">
  ...
  <TargetName>TheBestDistroEver</TargetName>
</PropertyGroup>
```

> Note: **DO NOT** change the ProjectName of the `DistroLauncher/DistroLauncher.vcxproj` from the value `launcher`. Doing so will break the build, as the DistroLauncher-Appx project is looking for the output of this project as `launcher.exe`.

6.  Update `MyDistro.appxmanifest`. There are several properties that are in the manifest that will need to be updated with your specific values:
    1. Note the `Identity Publisher` value (by default, `"CN=DistroOwner"`). We'll need that for testing the application.
    1. Ensure `<desktop:ExecutionAlias Alias="mydistro.exe" />` ends in ".exe". This is the command that will be used to launch your distro from the command line and should match the executable name we picked in step 4.
    1. Make sure each of the `Executable` values matches the executable name we picked in step 4.

7. Copy your tar.gz containing your distro into the root of the project and rename it to `install.tar.gz`.

## Setting up your Windows Environment
You will need a Windows environment to test that your app installs and works as expected. To set up a Windows environment for testing you can follow the steps from the [Windows Dev Center](https://developer.microsoft.com/en-us/windows/downloads/virtual-machines).

> Note: If you are using Hyper-V you can use the new VM gallery to easily spin up a Windows instance.

Also, to allow your locally built distro package to be manually side-loaded, ensure you've enabled Developer Mode in the Settings app (sideloading won't work without it). 

## Build and Test

To help building and testing the DistroLauncher project, we've included several scripts to automate some tasks. You can either choose to use these scripts from the command line, or work directly in Visual Studio, whatever your preference is. 

> **Note**: some sideloading/deployment steps don't work if you mix and match Visual Studio and the command line for development. If you run into errors while trying to deploy your app after already deploying it once, the easiest step is usually just to uninstall the previously sideloaded version and try again. 

### Building the Project (Command line):
To compile the project, you can simply type `build` in the root of the project to use MSBuild to build the solution. This is useful for verifying that your application compiles. It will also build an appx for you to sideload on your dev machine for testing.

> Note: We recommend that you build your launcher from the "Developer Command Prompt for Visual Studio" which can be launched from the start menu. This command-prompt sets up several path and environment variables to make building easier and smoother.

`build.bat` assumes that MSBuild is installed at one of the following paths:
`%ProgramFiles*%\MSBuild\14.0\bin\msbuild.exe` or
`%ProgramFiles*%\Microsoft Visual Studio\2017\Enterprise\MSBuild\15.0\Bin\msbuild.exe` or
`%ProgramFiles*%\Microsoft Visual Studio\2017\Community\MSBuild\15.0\Bin\msbuild.exe`.

If that's not the case, then you will need to modify that script.

Once you've completed the build, the packaged appx should be placed in a directory like `WSL-DistroLauncher\x64\Release\DistroLauncher-Appx` and should be named something like `DistroLauncher-Appx_1.0.0.0_x64.appx`. Simply double click that appx file to open the sideloading dialog. 

You can also use the PowerShell cmdlet `Add-AppxPackage` to register your appx:
``` powershell
powershell Add-AppxPackage x64\Debug\DistroLauncher-Appx\DistroLauncher-Appx_1.0.0.0_x64_Debug.appx
```

### Building Project (Visual Studio):

You can also easily build and deploy the distro launcher from Visual Studio. To sideload your appx on your machine for testing, all you need to do is right-click on the "Solution (DistroLauncher)" in the Solution Explorer and click "Deploy Solution". This should build the project and sideload it automatically for testing.

In order run your solution under the Visual Studio debugger, you will need to copy your install.tar.gz file into your output folder, for example: `x64\Debug`. **NOTE: If you have registered your distribution by this method, you will need to manually unregister it via wslconfig.exe /unregister**

### Installing & Testing
You should now have a finished appx sideloaded on your machine for testing.

To install your distro package, double click on the signed appx and click "Install". Note that this only installs the appx on your system - it doesn't unzip the tar.gz or register the distro yet. 

You should now find your distro in the Start menu, and you can launch your distro by clicking its Start menu tile or executing your distro from the command line by entering its name into a Cmd/PowerShell Console.

When you first run your newly installed distro, it is unpacked and registered with WSL. This can take a couple of minutes while all your distro files are unpacked and copied to your drive. 

Once complete, you should see a Console window with your distro running inside it.

### Publishing
If you are a distro vendor and want to publish  your distro to the Windows store, you will need to complete some pre-requisite steps to ensure the quality and integrity of the WSL distro ecosystem, and to safeguard our users:

#### Publishing Pre-Requisites
1. Reach out to the WSL team to introduce your distro, yourself, and your team
1. Agree with the WSL team on a testing and publishing plan
1. Complete any required paperwork
1. Sign up for an "Company" Windows Developer Account https://developer.microsoft.com/en-us/store/register. 
    > Note: This can take a week or more since you'll be required to confirm your organization's identity with an independent verification service via email and/or telephone.

#### Publishing Code changes
You'll also need to change a few small things in your project to prepare your distro for publishing to the Windows store

1. In your appxmanifest, you will need to change the values of the Identity field to match your identity in your Windows Store account:

``` xml
<Identity Name="1234YourCompanyName.YourAppName"
          Version="1.0.1.0"
          Publisher="CN=12345678-045C-ABCD-1234-ABCDEF987654"
          ProcessorArchitecture="x64" />
```

  > **NOTE**: Visual Studio can update this for you! You can do that by right-clicking on "DistroLauncher-Appx (Universal Windows)" in the solution explorer and clicking on "Store... Associate App with the Store..." and following the wizard. 

2. You will either need to run `build rel` from the command line to generate the Release version of your appx or use Visual Studio directly to upload your package to the store. You can do this by right-clicking on "DistroLauncher-Appx (Universal Windows)" in the solution explorer and clicking on "Store... Create App Packages..." and following the wizard.

Also, make sure to check out the [Notes for uploading to the Store](https://github.com/Microsoft/WSL-DistroLauncher/wiki/Notes-for-uploading-to-the-Store) page on our wiki for more information.

# Issues & Contact
Any bugs or problems discovered with the Launcher should be filed in this project's Issues list. The team will be notified and will respond to the reported issue within 3 (US) working days.

You may also reach out to our team alias at wslpartners@microsoft.com for questions related to submitting your app to the Microsoft Store.

# Contributing
This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/). For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.
