LanguageNames = (English=0x409:MSG00409)

MessageId=1001 SymbolicName=MSG_WSL_REGISTER_DISTRIBUTION_FAILED
Language=English
WslRegisterDistribution failed with error: 0x%1!x!
.

MessageId=1002 SymbolicName=MSG_WSL_CONFIGURE_DISTRIBUTION_FAILED
Language=English
WslGetDistributionConfiguration failed with error: 0x%1!x!
.

MessageId=1003 SymbolicName=MSG_WSL_LAUNCH_INTERACTIVE_FAILED
Language=English
WslLaunchInteractive %1 failed with error: 0x%2!x!
.

MessageId=1004 SymbolicName=MSG_WSL_LAUNCH_FAILED
Language=English
WslLaunch %1 failed with error: 0x%2!x!
.

MessageId=1005 SymbolicName=MSG_USAGE
Language=English
Launches or configures a Linux distribution.

Usage: 
    <no args> 
        Launches the user's default shell in the user's home directory.

    install [--root]
        Install the distribuiton and do not launch the shell when complete.
          --root
              Do not create a user account and leave the default user set to root.

    run <command line> 
        Run the provided command line in the current working directory. If no
        command line is provided, the default shell is launched.

    config [setting [value]] 
        Configure settings for this distribution.
        Settings:
          --default-user <username>
              Sets the default user to <username>. This must be an existing user.

    help 
        Print usage information.
.

MessageId=1006 SymbolicName=MSG_STATUS_INSTALLING
Language=English
Installing, this may take a few minutes...
.

MessageId=1007 SymbolicName=MSG_INSTALL_SUCCESS
Language=English
Installation successful!
.

MessageId=1008 SymbolicName=MSG_ERROR_CODE
Language=English
Error: 0x%1!x! %2
.

MessageId=1009 SymbolicName=MSG_ENTER_USERNAME
Language=English
Enter new UNIX username: %0
.

MessageId=1010 SymbolicName=MSG_CREATE_USER_PROMPT
Language=English
Please create a default UNIX user account. The username does not need to match your Windows username.
For more information visit: https://aka.ms/wslusers
.

MessageId=1011 SymbolicName=MSG_PRESS_A_KEY
Language=English
Press any key to continue...
.

MessageId=1012 SymbolicName=MSG_MISSING_OPTIONAL_COMPONENT
Language=English
The Windows Subsystem for Linux optional component is not enabled. Please enable it and try again.
See https://aka.ms/wslinstall for details.
.

MessageId=1013 SymbolicName=MSG_INSTALL_ALREADY_EXISTS
Language=English
The distribution installation has become corrupted.
Please select Reset from App Settings or uninstall and reinstall the app.
.
