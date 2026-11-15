using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using WinRT.Interop;

namespace GlideDock.Setup;

public partial class App : Application
{
    private SetupWindow? _window;
    private readonly LaunchRequest _request;

    public App()
    {
        InitializeComponent();
        _request = LaunchRequest.Parse(Environment.GetCommandLineArgs().Skip(1));
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        if (_request.Mode == LaunchMode.InstallWorker)
        {
            _ = RunInstallWorkerAsync();
            return;
        }

        _window = new SetupWindow(_request);
        _window.Activate();
        SetWindowIcon(_window);
    }

    private async Task RunInstallWorkerAsync()
    {
        try
        {
            var options = _request.WorkerOptions
                ?? throw new InvalidDataException("安装参数缺失。");
            await InstallerEngine.InstallAsync(
                options,
                Environment.ProcessPath ?? throw new InvalidOperationException("无法定位安装程序。"),
                machineScope: true,
                CancellationToken.None);
            Exit();
        }
        catch (Exception error)
        {
            NativeMethods.MessageBox(IntPtr.Zero, error.Message, "Glide-Dock 安装失败", 0x10);
            Environment.Exit(1);
        }
    }

    private static void SetWindowIcon(Window window)
    {
        try
        {
            var hwnd = WindowNative.GetWindowHandle(window);
            var id = Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(id);
            var exe = Environment.ProcessPath;
            if (!string.IsNullOrWhiteSpace(exe) && File.Exists(exe))
            {
                appWindow.SetIcon(exe);
            }
        }
        catch
        {
            // The window remains usable if Windows declines a custom icon.
        }
    }
}

internal enum LaunchMode
{
    Interactive,
    Update,
    Uninstall,
    InstallWorker,
    UninstallWorker
}

internal sealed record LaunchRequest(
    LaunchMode Mode,
    InstallOptions? WorkerOptions = null,
    UninstallRecord? Uninstall = null,
    int ParentProcessId = 0)
{
    public static LaunchRequest Parse(IEnumerable<string> arguments)
    {
        var args = arguments.ToArray();
        for (var index = 0; index < args.Length; index++)
        {
            switch (args[index])
            {
                case "--update":
                    return new LaunchRequest(LaunchMode.Update);
                case "--uninstall":
                    return new LaunchRequest(LaunchMode.Uninstall);
                case "--install-worker" when index + 1 < args.Length:
                    return new LaunchRequest(
                        LaunchMode.InstallWorker,
                        WorkerOptions: InstallOptions.FromBase64(args[index + 1]));
                case "--uninstall-worker" when index + 2 < args.Length:
                    return new LaunchRequest(
                        LaunchMode.UninstallWorker,
                        Uninstall: UninstallRecord.FromBase64(args[index + 1]),
                        ParentProcessId: int.TryParse(args[index + 2], out var pid) ? pid : 0);
            }
        }
        return new LaunchRequest(LaunchMode.Interactive);
    }
}

internal static class NativeMethods
{
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    internal static extern int MessageBox(IntPtr owner, string text, string caption, uint type);
}
