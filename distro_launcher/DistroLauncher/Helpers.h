//
//    Copyright (C) Microsoft.  All rights reserved.
// Licensed under the terms described in the LICENSE file in the root of this project.
//

#pragma once

#define UID_INVALID ((ULONG)-1)

namespace Helpers
{
    std::wstring GetUserInput(DWORD promptMsg, DWORD maxCharacters);
    void PrintErrorMessage(HRESULT hr);
    HRESULT PrintMessage(DWORD messageId, ...);
    void PromptForInput();
}