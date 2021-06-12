//
//    Copyright (C) Microsoft.  All rights reserved.
// Licensed under the terms described in the LICENSE file in the root of this project.
//

#pragma once
#include <wslapi.h>

// This error definition is present in the Spring Creators Update SDK.
#ifndef ERROR_LINUX_SUBSYSTEM_NOT_PRESENT
#define ERROR_LINUX_SUBSYSTEM_NOT_PRESENT 414L
#endif // !ERROR_LINUX_SUBSYSTEM_NOT_PRESENT

typedef BOOL    (STDAPICALLTYPE* WSL_IS_DISTRIBUTION_REGISTERED)(PCWSTR);
typedef HRESULT (STDAPICALLTYPE* WSL_REGISTER_DISTRIBUTION)(PCWSTR, PCWSTR);
typedef HRESULT (STDAPICALLTYPE* WSL_CONFIGURE_DISTRIBUTION)(PCWSTR, ULONG, WSL_DISTRIBUTION_FLAGS);
typedef HRESULT (STDAPICALLTYPE* WSL_GET_DISTRIBUTION_CONFIGURATION)(PCWSTR, ULONG *, ULONG *, WSL_DISTRIBUTION_FLAGS *, PSTR **, ULONG *);
typedef HRESULT (STDAPICALLTYPE* WSL_LAUNCH_INTERACTIVE)(PCWSTR, PCWSTR, BOOL, DWORD *);
typedef HRESULT (STDAPICALLTYPE* WSL_LAUNCH)(PCWSTR, PCWSTR, BOOL, HANDLE, HANDLE, HANDLE, HANDLE *);

class WslApiLoader
{
  public:
    WslApiLoader(const std::wstring& distributionName);
    ~WslApiLoader();

    BOOL WslIsOptionalComponentInstalled();

    BOOL WslIsDistributionRegistered();

    HRESULT WslRegisterDistribution();

    HRESULT WslConfigureDistribution(ULONG defaultUID,
                                     WSL_DISTRIBUTION_FLAGS wslDistributionFlags);

    HRESULT WslLaunchInteractive(PCWSTR command,
                                 BOOL useCurrentWorkingDirectory,
                                 DWORD *exitCode);

    HRESULT WslLaunch(PCWSTR command,
                      BOOL useCurrentWorkingDirectory,
                      HANDLE stdIn,
                      HANDLE stdOut,
                      HANDLE stdErr,
                      HANDLE *process);

  private:
    std::wstring _distributionName;
    HMODULE _wslApiDll;
    WSL_IS_DISTRIBUTION_REGISTERED _isDistributionRegistered;
    WSL_REGISTER_DISTRIBUTION _registerDistribution;
    WSL_CONFIGURE_DISTRIBUTION _configureDistribution;
    WSL_LAUNCH_INTERACTIVE _launchInteractive;
    WSL_LAUNCH _launch;
};

extern WslApiLoader g_wslApi;
