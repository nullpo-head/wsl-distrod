//
//    Copyright (C) Microsoft.  All rights reserved.
// Licensed under the terms described in the LICENSE file in the root of this project.
//

#pragma once

namespace DistributionInfo
{
    // The name of the distribution. This will be displayed to the user via
    // wslconfig.exe and in other places. It must conform to the following
    // regular expression: ^[a-zA-Z0-9._-]+$
    //
    // WARNING: This value must not change between versions of your app,
    // otherwise users upgrading from older versions will see launch failures.
    const std::wstring Name = L"MyDistribution";

    // The title bar for the console window while the distribution is installing.
    const std::wstring WindowTitle = L"My Distribution";

    // Create and configure a user account.
    bool CreateUser(std::wstring_view userName);

    // Query the UID of the user account.
    ULONG QueryUid(std::wstring_view userName);
}