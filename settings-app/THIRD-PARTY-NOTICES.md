# Third-party notices

Glide-Dock's own source is licensed under MIT OR Apache-2.0. The settings center uses
the following Microsoft components:

## Windows App SDK / WinUI 3

- Package: `Microsoft.WindowsAppSDK` 1.7.260224002
- Copyright: Microsoft Corporation
- License: Microsoft Windows App SDK license terms, reproduced in
  [`licenses/WindowsAppSDK-license.txt`](licenses/WindowsAppSDK-license.txt)
  and copied next to the published executable as `WindowsAppSDK-license.txt`.
- Project: https://github.com/microsoft/WindowsAppSDK

## .NET runtime

The self-contained settings publish includes the .NET 8 runtime, licensed under the
MIT License. See https://github.com/dotnet/runtime/blob/main/LICENSE.TXT.

## Noto Sans SC

`Assets/Fonts/NotoSansSC-VF.ttf` is Noto Sans SC, distributed under the SIL Open Font
License 1.1. The full license is included at `licenses/OFL-1.1.txt`. The font is bundled
as an application asset and is not installed into the Windows system font directory.

## Other dependencies

The Windows App SDK package includes Microsoft WebView2 and Windows SDK runtime
components. Their notices are included in the NuGet package and apply to the
self-contained output as well. Keep this file and the license text when
redistributing `settings-app/publish`.
