//
//    Copyright (C) Microsoft.  All rights reserved.
// Licensed under the terms described in the LICENSE file in the root of this project.
//

#include "stdafx.h"
#include "WslApiLoader.h"

WslApiLoader::WslApiLoader(const std::wstring& distributionName) :
    _distributionName(distributionName)
{
    _wslApiDll = LoadLibraryEx(L"wslapi.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (_wslApiDll != nullptr) {
        _isDistributionRegistered = (WSL_IS_DISTRIBUTION_REGISTERED)GetProcAddress(_wslApiDll, "WslIsDistributionRegistered");
        _registerDistribution = (WSL_REGISTER_DISTRIBUTION)GetProcAddress(_wslApiDll, "WslRegisterDistribution");
        _configureDistribution = (WSL_CONFIGURE_DISTRIBUTION)GetProcAddress(_wslApiDll, "WslConfigureDistribution");
        _launchInteractive = (WSL_LAUNCH_INTERACTIVE)GetProcAddress(_wslApiDll, "WslLaunchInteractive");
        _launch = (WSL_LAUNCH)GetProcAddress(_wslApiDll, "WslLaunch");
    }
}

WslApiLoader::~WslApiLoader()
{
    if (_wslApiDll != nullptr) {
        FreeLibrary(_wslApiDll);
    }
}

BOOL WslApiLoader::WslIsOptionalComponentInstalled()
{
    return ((_wslApiDll != nullptr) && 
            (_isDistributionRegistered != nullptr) &&
            (_registerDistribution != nullptr) &&
            (_configureDistribution != nullptr) &&
            (_launchInteractive != nullptr) &&
            (_launch != nullptr));
}

BOOL WslApiLoader::WslIsDistributionRegistered()
{
    return _isDistributionRegistered(_distributionName.c_str());
}

HRESULT WslApiLoader::WslRegisterDistribution()
{
    HRESULT hr = _registerDistribution(_distributionName.c_str(), L"install.tar.gz");
    if (FAILED(hr)) {
        Helpers::PrintMessage(MSG_WSL_REGISTER_DISTRIBUTION_FAILED, hr);
    }

    return hr;
}

HRESULT WslApiLoader::WslConfigureDistribution(ULONG defaultUID, WSL_DISTRIBUTION_FLAGS wslDistributionFlags)
{
    HRESULT hr = _configureDistribution(_distributionName.c_str(), defaultUID, wslDistributionFlags);
    if (FAILED(hr)) {
        Helpers::PrintMessage(MSG_WSL_CONFIGURE_DISTRIBUTION_FAILED, hr);
    }

    return hr;
}

HRESULT WslApiLoader::WslLaunchInteractive(PCWSTR command, BOOL useCurrentWorkingDirectory, DWORD *exitCode)
{
    HRESULT hr = _launchInteractive(_distributionName.c_str(), command, useCurrentWorkingDirectory, exitCode);
    if (FAILED(hr)) {
        Helpers::PrintMessage(MSG_WSL_LAUNCH_INTERACTIVE_FAILED, command, hr);
    }

    return hr;
}

HRESULT WslApiLoader::WslLaunch(PCWSTR command, BOOL useCurrentWorkingDirectory, HANDLE stdIn, HANDLE stdOut, HANDLE stdErr, HANDLE *process)
{
    HRESULT hr = _launch(_distributionName.c_str(), command, useCurrentWorkingDirectory, stdIn, stdOut, stdErr, process);
    if (FAILED(hr)) {
        Helpers::PrintMessage(MSG_WSL_LAUNCH_FAILED, command, hr);
    }

    return hr;
}
