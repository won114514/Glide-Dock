using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.UI;
using Windows.Storage.Pickers;
using WinRT.Interop;

namespace GlideDock.Setup;

internal sealed partial class SetupWindow
{
    private async void Next_Click(object sender, RoutedEventArgs e)
    {
        if (_busy) return;
        switch (_page)
        {
            case WizardPage.Welcome:
                if (!_preflightPassed) return;
                if (_existing is not null)
                {
                    _options = OptionsForUpdate(_existing);
                    await RunUpdateAsync();
                    return;
                }
                _page = WizardPage.Install;
                break;
            case WizardPage.Install:
                if (!ReadInstallOptions()) return;
                _page = WizardPage.FirstRun;
                break;
            case WizardPage.FirstRun:
                ReadFirstRunOptions();
                await RunFreshInstallAsync();
                return;
            case WizardPage.Uninstall:
                await StartUninstallAsync();
                return;
            case WizardPage.Finished:
                if (_completed && _launchCheck?.IsChecked == true)
                {
                    InstallerEngine.LaunchInstalledDock(_options.InstallDirectory);
                }
                Close();
                return;
        }
        _errorText = null;
        RenderPage();
    }

    private void Back_Click(object sender, RoutedEventArgs e)
    {
        if (_busy) return;
        if (_page == WizardPage.Install) _page = WizardPage.Welcome;
        else if (_page == WizardPage.FirstRun) _page = WizardPage.Install;
        _errorText = null;
        RenderPage();
    }

    private void Cancel_Click(object sender, RoutedEventArgs e)
    {
        if (!_busy) Close();
    }

    private bool ReadInstallOptions()
    {
        try
        {
            var scopeValue = (_scopeBox?.SelectedItem as ComboBoxItem)?.Tag;
            var scope = scopeValue is InstallScope value ? value : InstallScope.AllUsers;
            var path = _installPathBox?.Text.Trim() ?? string.Empty;
            InstallerEngine.ValidateInstallDirectory(path, update: false);
            InstallerEngine.ValidateInstallCapacity(path);
            _options = _options with
            {
                Scope = scope,
                InstallDirectory = Path.GetFullPath(path),
                CreateDesktopShortcut = _desktopCheck?.IsChecked == true,
                OwnerSid = InstallerEngine.CurrentOwnerSid()
            };
            _errorText = null;
            return true;
        }
        catch (Exception error)
        {
            ShowInputError(error.Message);
            return false;
        }
    }

    private void ReadFirstRunOptions()
    {
        var theme = _themeChoices?.Children.OfType<RadioButton>()
            .FirstOrDefault(button => button.IsChecked == true)?.Tag as string ?? "system";
        _options = _options with
        {
            Theme = theme,
            EnableStartup = _startupCheck?.IsChecked == true,
            SelectedApps = _appChecks.Where(pair => pair.Value.IsChecked == true)
                .Select(pair => pair.Key).ToList()
        };
    }

    private async Task RunFreshInstallAsync()
    {
        _options = _options with { IsUpdate = false };
        _errorText = null;
        _page = WizardPage.Progress;
        _busy = true;
        RenderPage();
        try
        {
            await InstallFromUiAsync(_options);
            _completed = true;
            _page = WizardPage.Finished;
        }
        catch (Exception error)
        {
            _errorText = ErrorBlock(error.Message);
            _page = WizardPage.FirstRun;
        }
        finally
        {
            _busy = false;
            RenderPage();
        }
    }

    private async Task RunAutomaticUpdateAsync()
    {
        if (_existing is null)
        {
            _page = WizardPage.Progress;
            RenderPage();
            ShowInputError("没有找到可更新的 Glide-Dock 安装。自动更新不会执行首次安装。");
            NextButton.Visibility = Visibility.Collapsed;
            CancelButton.Visibility = Visibility.Collapsed;
            return;
        }
        _options = OptionsForUpdate(_existing);
        await RunUpdateAsync();
    }

    private InstallOptions OptionsForUpdate(UninstallRecord record) => new()
    {
        Scope = record.Scope,
        InstallDirectory = record.InstallDirectory,
        OwnerSid = record.OwnerSid,
        IsUpdate = true,
        LaunchAfterInstall = false,
        EnableStartup = false,
        CreateDesktopShortcut = false,
        Theme = "system",
        SelectedApps = []
    };

    private async Task RunUpdateAsync()
    {
        _errorText = null;
        _page = WizardPage.Progress;
        _busy = true;
        RenderPage();
        try
        {
            await InstallFromUiAsync(_options);
            _completed = true;
            _page = WizardPage.Finished;
        }
        catch (Exception error)
        {
            _errorText = ErrorBlock(error.Message);
            _page = WizardPage.Welcome;
        }
        finally
        {
            _busy = false;
            RenderPage();
        }
    }

    private async Task InstallFromUiAsync(InstallOptions options)
    {
        var restartDock = options.IsUpdate && InstallerEngine.IsDockRunning();
        var progress = new Progress<InstallProgress>(UpdateInstallProgress);
        if (options.Scope == InstallScope.AllUsers)
        {
            await RunElevatedInstallAsync(options, progress);
        }
        else
        {
            await InstallerEngine.InstallAsync(
                options, CurrentSetupPath, machineScope: false, CancellationToken.None, progress);
        }

        if (!options.IsUpdate && !_hasExistingConfig)
        {
            try
            {
                InstallerEngine.ConfigureUserProfile(options);
            }
            catch (Exception error)
            {
                _errorText = ErrorBlock("Glide-Dock 已安装，但首次配置未能全部保存：" + error.Message);
            }
        }
        if (restartDock)
        {
            InstallerEngine.LaunchInstalledDock(options.InstallDirectory);
        }
    }

    private async Task RunElevatedInstallAsync(
        InstallOptions options,
        IProgress<InstallProgress> progress)
    {
        var pipeName = "GlideDock-Install-" + Guid.NewGuid().ToString("N");
        await using var pipe = new NamedPipeServerStream(
            pipeName,
            PipeDirection.In,
            1,
            PipeTransmissionMode.Byte,
            PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly);
        using var process = StartWorker(
            CurrentSetupPath, "--install-worker", options.ToBase64(), elevate: true, extra: pipeName);

        var connection = pipe.WaitForConnectionAsync();
        var processExit = process.WaitForExitAsync();
        var first = await Task.WhenAny(connection, processExit);
        if (first == processExit && !pipe.IsConnected)
        {
            await processExit;
            throw new InvalidOperationException(
                process.ExitCode == 0
                    ? "安装工作进程提前退出，安装没有执行。"
                    : "安装工作进程未能启动。请检查管理员授权后重试。");
        }
        await connection;

        using var reader = new StreamReader(
            pipe, System.Text.Encoding.UTF8, detectEncodingFromByteOrderMarks: false,
            bufferSize: 1024, leaveOpen: true);
        string? workerError = null;
        while (await reader.ReadLineAsync() is { } line)
        {
            InstallProgress? update;
            try
            {
                update = JsonSerializer.Deserialize<InstallProgress>(line);
            }
            catch (JsonException error)
            {
                throw new InvalidDataException("管理员安装进程返回了无效的进度数据。", error);
            }
            if (update is null) continue;
            if (update.IsError) workerError = update.Message;
            progress.Report(update);
        }

        await process.WaitForExitAsync();
        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException(workerError ?? "安装未能完成，请检查权限和磁盘状态后重试。");
        }
    }

    private void UpdateInstallProgress(InstallProgress progress)
    {
        if (_statusText is not null) _statusText.Text = progress.Message;
        if (_progressBar is not null) _progressBar.Value = Math.Clamp(progress.Percent, 0, 100);
        if (_progressPercentText is not null) _progressPercentText.Text = $"{Math.Clamp(progress.Percent, 0, 100)}%";
    }

    private async Task RunStartupPreflightAsync()
    {
        _page = WizardPage.Preflight;
        _errorText = null;
        RenderPage();
        try
        {
            var directory = _existing?.InstallDirectory
                ?? InstallerEngine.DefaultInstallDirectory(InstallScope.AllUsers);
            _environmentChecks = await Task.Run(() => InstallerEngine.CheckEnvironment(directory));
            _preflightPassed = _environmentChecks.All(check => check.State != CheckState.Failed);
            if (!_preflightPassed)
            {
                _errorText = ErrorBlock("环境检查未通过。请确认系统要求并重新下载完整安装包。");
            }
        }
        catch (Exception error)
        {
            _preflightPassed = false;
            _errorText = ErrorBlock("无法完成启动环境检查：" + error.Message);
        }
        finally
        {
            _page = WizardPage.Welcome;
            RenderPage();
        }
    }

    private Task StartUninstallAsync()
    {
        if (_existing is null) return Task.CompletedTask;
        try
        {
            var tempDirectory = Path.Combine(Path.GetTempPath(), "Glide-Dock-Uninstall", Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(tempDirectory);
            var tempExe = Path.Combine(tempDirectory, "GlideDock.Setup.exe");
            File.Copy(CurrentSetupPath, tempExe);
            StartWorker(tempExe, "--uninstall-worker", _existing.ToBase64(),
                elevate: _existing.Scope == InstallScope.AllUsers,
                Environment.ProcessId.ToString());
            Close();
        }
        catch (Exception error)
        {
            ShowInputError(error.Message);
        }
        return Task.CompletedTask;
    }

    private async Task RunUninstallWorkerAsync()
    {
        if (_request.Uninstall is null)
        {
            ShowInputError("卸载参数缺失。");
            return;
        }
        _page = WizardPage.UninstallProgress;
        _busy = true;
        RenderPage();
        try
        {
            var progress = new Progress<string>(message =>
            {
                if (_statusText is not null) _statusText.Text = message;
            });
            await InstallerEngine.UninstallAsync(
                _request.Uninstall,
                _request.ParentProcessId,
                progress,
                CancellationToken.None);
            _completed = false;
            _page = WizardPage.Finished;
        }
        catch (Exception error)
        {
            _errorText = ErrorBlock(error.Message);
            _page = WizardPage.Uninstall;
        }
        finally
        {
            _busy = false;
            RenderPage();
        }
    }

    private static Process StartWorker(
        string executable, string mode, string data, bool elevate, string? extra = null)
    {
        var start = new ProcessStartInfo
        {
            FileName = executable,
            UseShellExecute = elevate,
            WorkingDirectory = Path.GetDirectoryName(executable) ?? Environment.CurrentDirectory
        };
        if (elevate) start.Verb = "runas";
        start.ArgumentList.Add(mode);
        start.ArgumentList.Add(data);
        if (extra is not null) start.ArgumentList.Add(extra);
        try
        {
            return Process.Start(start) ?? throw new InvalidOperationException("无法启动安装工作进程。");
        }
        catch (System.ComponentModel.Win32Exception error) when (error.NativeErrorCode == 1223)
        {
            throw new OperationCanceledException("已取消管理员授权。", error);
        }
    }

    private void Scope_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_scopeBox?.SelectedItem is not ComboBoxItem item || _installPathBox is null) return;
        var scope = item.Tag is InstallScope value ? value : InstallScope.AllUsers;
        var oldDefault = InstallerEngine.DefaultInstallDirectory(_options.Scope);
        if (string.Equals(_installPathBox.Text, oldDefault, StringComparison.OrdinalIgnoreCase))
        {
            _installPathBox.Text = InstallerEngine.DefaultInstallDirectory(scope);
        }
        _options = _options with { Scope = scope };
    }

    private async void Browse_Click(object sender, RoutedEventArgs e)
    {
        var picker = new FolderPicker { SuggestedStartLocation = PickerLocationId.ComputerFolder };
        picker.FileTypeFilter.Add("*");
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        var folder = await picker.PickSingleFolderAsync();
        if (folder is null || _installPathBox is null) return;
        _installPathBox.Text = string.Equals(Path.GetFileName(folder.Path), "Glide-Dock", StringComparison.OrdinalIgnoreCase)
            ? folder.Path
            : Path.Combine(folder.Path, "Glide-Dock");
    }

    private void ShowInputError(string message)
    {
        _errorText = ErrorBlock(message);
        PageHost.Children.Add(_errorText);
    }

    private static TextBlock ErrorBlock(string message) => new()
    {
        Text = message,
        TextWrapping = TextWrapping.Wrap,
        Foreground = new SolidColorBrush(Color.FromArgb(255, 177, 48, 48)),
        Margin = new Thickness(0, 12, 0, 0)
    };
}
