using System;
using System.IO;
using Microsoft.UI.Xaml;
using WinRT.Interop;

namespace GlideDock.Settings;

public partial class App : Application
{
    private Window? _window;

    public static string ConfigPath { get; private set; } = string.Empty;
    public static string DockExecutablePath { get; private set; } = string.Empty;
    public static IntPtr OwnerWindow { get; private set; }

    public App()
    {
        InitializeComponent();
        ParseArguments();
    }

    private static void ParseArguments()
    {
        var defaultPath = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "Glide-Dock",
            "config.json");
        ConfigPath = defaultPath;
        DockExecutablePath = string.Empty;
        OwnerWindow = IntPtr.Zero;

        var args = Environment.GetCommandLineArgs();
        for (var index = 1; index < args.Length; index++)
        {
            switch (args[index])
            {
                case "--config" when index + 1 < args.Length:
                    ConfigPath = args[++index];
                    break;
                case "--owner" when index + 1 < args.Length:
                    if (long.TryParse(args[++index], out var owner))
                    {
                        OwnerWindow = new IntPtr(owner);
                    }
                    break;
                case "--dock-exe" when index + 1 < args.Length:
                    DockExecutablePath = args[++index];
                    break;
            }
        }

        ConfigPath = NormalizePath(ConfigPath);
        DockExecutablePath = NormalizePath(DockExecutablePath);
        ResolveDockExecutable();
    }

    private static string NormalizePath(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return path;
        }
        if (path.StartsWith(@"\\?\UNC\", StringComparison.Ordinal))
        {
            return @"\\" + path.Substring(8);
        }
        if (path.StartsWith(@"\\?\", StringComparison.Ordinal))
        {
            return path.Substring(4);
        }
        return path;
    }

    private static void ResolveDockExecutable()
    {
        if (!string.IsNullOrWhiteSpace(DockExecutablePath)
            && File.Exists(DockExecutablePath))
        {
            DockExecutablePath = Path.GetFullPath(DockExecutablePath);
            return;
        }

        var candidate = Path.GetFullPath(
            Path.Combine(AppContext.BaseDirectory, "..", "glide-dock.exe"));
        DockExecutablePath = File.Exists(candidate) ? candidate : string.Empty;
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        try
        {
            _window = new MainWindow(ConfigPath, OwnerWindow);
            _window.Activate();
            var hwnd = WindowNative.GetWindowHandle(_window);
            NativeMethods.SetWindowPos(
                hwnd,
                IntPtr.Zero,
                0,
                0,
                0,
                0,
                NativeMethods.SWP_NOMOVE | NativeMethods.SWP_NOSIZE | NativeMethods.SWP_NOACTIVATE);
            NativeMethods.SetForegroundWindow(hwnd);
        }
        catch (Exception error)
        {
            NativeMethods.MessageBox(
                IntPtr.Zero,
                "无法打开 Glide-Dock 设置：\n\n" + error.Message,
                "Glide-Dock 设置",
                0x00000010);
            Exit();
        }
    }
}
