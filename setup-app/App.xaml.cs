using System.Runtime.InteropServices;
using System.IO.Pipes;
using System.Text;
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
        NamedPipeClientStream? pipe = null;
        try
        {
            var options = _request.WorkerOptions
                ?? throw new InvalidDataException("安装参数缺失。");
            var pipeName = _request.ProgressPipeName
                ?? throw new InvalidDataException("安装进度通道缺失，请重新启动安装程序。");
            pipe = new NamedPipeClientStream(".", pipeName, PipeDirection.Out, PipeOptions.Asynchronous);
            await pipe.ConnectAsync(30000);
            await using var writer = new StreamWriter(
                pipe, Encoding.UTF8, bufferSize: 1024, leaveOpen: true) { AutoFlush = true };
            var progress = new PipeInstallProgress(writer);
            try
            {
                await InstallerEngine.InstallAsync(
                    options,
                    Environment.ProcessPath ?? throw new InvalidOperationException("无法定位安装程序。"),
                    machineScope: true,
                    CancellationToken.None,
                    progress);
            }
            catch (Exception error)
            {
                progress.Report(new InstallProgress(0, error.Message, IsError: true));
                Environment.Exit(1);
                return;
            }
            Exit();
        }
        catch (Exception error)
        {
            NativeMethods.MessageBox(IntPtr.Zero, error.Message, "Glide-Dock 安装失败", 0x10);
            Environment.Exit(1);
        }
        finally
        {
            pipe?.Dispose();
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
    int ParentProcessId = 0,
    string? ProgressPipeName = null)
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
                case "--install-worker" when index + 2 < args.Length:
                    return new LaunchRequest(
                        LaunchMode.InstallWorker,
                        WorkerOptions: InstallOptions.FromBase64(args[index + 1]),
                        ProgressPipeName: args[index + 2]);
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
