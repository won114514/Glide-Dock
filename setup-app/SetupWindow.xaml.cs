using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using Microsoft.UI;
using Windows.UI;
using WinRT.Interop;

namespace GlideDock.Setup;

internal sealed partial class SetupWindow : Window
{
    private enum WizardPage { Preflight, Welcome, Install, FirstRun, Progress, Uninstall, UninstallProgress, Finished }

    private readonly LaunchRequest _request;
    private readonly UninstallRecord? _existing;
    private readonly IReadOnlyList<AppCandidate> _availableApps = AppCandidate.Discover();
    private readonly Dictionary<string, CheckBox> _appChecks = new(StringComparer.Ordinal);
    private InstallOptions _options;
    private WizardPage _page;
    private bool _busy;
    private bool _completed;
    private bool _preflightPassed;
    private bool _hasExistingConfig;
    private IReadOnlyList<EnvironmentCheck> _environmentChecks = Array.Empty<EnvironmentCheck>();
    private TextBox? _installPathBox;
    private ComboBox? _scopeBox;
    private StackPanel? _themeChoices;
    private CheckBox? _startupCheck;
    private CheckBox? _desktopCheck;
    private CheckBox? _launchCheck;
    private ProgressRing? _progressRing;
    private ProgressBar? _progressBar;
    private TextBlock? _progressPercentText;
    private TextBlock? _statusText;
    private TextBlock? _errorText;

    public SetupWindow(LaunchRequest request)
    {
        _request = request;
        _existing = InstallerEngine.FindInstall();
        _hasExistingConfig = File.Exists(Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "Glide-Dock", "config.json"));
        _options = NewOptions();
        _page = request.Mode switch
        {
            LaunchMode.Uninstall or LaunchMode.UninstallWorker => WizardPage.Uninstall,
            _ => WizardPage.Preflight
        };

        InitializeComponent();
        RootGrid.RequestedTheme = ElementTheme.Light;
        VersionText.Text = "版本 " + ProductVersion;
        Title = request.Mode == LaunchMode.UninstallWorker ? "正在卸载 Glide-Dock" : "Glide-Dock 安装程序";
        ConfigureWindow();
        RootGrid.Loaded += SetupWindow_Loaded;
        RenderPage();
    }

    private string ProductVersion => typeof(App).Assembly.GetName().Version?.ToString(3) ?? "0.4.1";
    private static string CurrentSetupPath => Environment.ProcessPath
        ?? throw new InvalidOperationException("无法定位安装程序。");

    private InstallOptions NewOptions() => new()
    {
        Scope = InstallScope.AllUsers,
        InstallDirectory = InstallerEngine.DefaultInstallDirectory(InstallScope.AllUsers),
        EnableStartup = false,
        Theme = "system",
        SelectedApps = _availableApps.Select(app => app.Id).ToList(),
        OwnerSid = InstallerEngine.CurrentOwnerSid()
    };

    private void ConfigureWindow()
    {
        var hwnd = WindowNative.GetWindowHandle(this);
        var id = Win32Interop.GetWindowIdFromWindow(hwnd);
        var window = AppWindow.GetFromWindowId(id);
        const int width = 1020;
        const int height = 700;
        window.Resize(new Windows.Graphics.SizeInt32(width, height));
        var workArea = DisplayArea.GetFromWindowId(id, DisplayAreaFallback.Primary).WorkArea;
        window.Move(new Windows.Graphics.PointInt32(
            Math.Max(0, (workArea.Width - width) / 2),
            Math.Max(0, (workArea.Height - height) / 2)));
        if (window.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = true;
        }
    }

    private async void SetupWindow_Loaded(object sender, RoutedEventArgs e)
    {
        BrandLogo.Source = await LoadImageAsync("GlideDock.Setup.Brand.Logo.png");
        if (_request.Mode is not LaunchMode.Uninstall and not LaunchMode.UninstallWorker)
        {
            await RunStartupPreflightAsync();
        }
        if (_request.Mode == LaunchMode.Update)
        {
            if (_preflightPassed) await RunAutomaticUpdateAsync();
        }
        else if (_request.Mode == LaunchMode.UninstallWorker)
        {
            await RunUninstallWorkerAsync();
        }
    }

    private static Task<BitmapImage?> LoadImageAsync(string name)
    {
        var fileName = name switch
        {
            "GlideDock.Setup.Brand.Logo.png" => "Glide-Dock-Logo-Horizontal.png",
            "GlideDock.Setup.Brand.Preview.png" => "Glide-Dock-Brand-Preview.png",
            _ => throw new ArgumentOutOfRangeException(nameof(name))
        };
        var path = Path.Combine(AppContext.BaseDirectory, "Assets", fileName);
        return Task.FromResult(File.Exists(path)
            ? new BitmapImage(new Uri(path))
            : null);
    }
}
