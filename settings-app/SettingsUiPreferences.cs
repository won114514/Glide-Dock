using System.Text.Json;

namespace GlideDock.Settings;

internal sealed class SettingsUiPreferences
{
    public const string SystemFont = "system";
    public const string BundledFont = "noto-sans-sc";

    private static readonly JsonSerializerOptions WriteOptions = new() { WriteIndented = true };

    private readonly string _path;

    private SettingsUiPreferences(string path, string fontId)
    {
        _path = path;
        FontId = fontId;
    }

    public string FontId { get; private set; }

    public static SettingsUiPreferences Load(string configPath)
    {
        var directory = Path.GetDirectoryName(Path.GetFullPath(configPath)) ?? AppContext.BaseDirectory;
        var path = Path.Combine(directory, "settings-ui.json");
        try
        {
            if (File.Exists(path))
            {
                var stored = JsonSerializer.Deserialize<PreferencesFile>(File.ReadAllText(path));
                var fontId = stored is { Version: 1, FontId: SystemFont } ? SystemFont : BundledFont;
                return new SettingsUiPreferences(path, fontId);
            }
        }
        catch (JsonException)
        {
        }
        catch (IOException)
        {
        }
        catch (UnauthorizedAccessException)
        {
        }

        return new SettingsUiPreferences(path, BundledFont);
    }

    public void SetFont(string fontId)
    {
        var selectedFont = fontId == SystemFont ? SystemFont : BundledFont;
        var directory = Path.GetDirectoryName(_path);
        if (!string.IsNullOrEmpty(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var temporaryPath = _path + ".tmp";
        try
        {
            var bytes = JsonSerializer.SerializeToUtf8Bytes(
                new PreferencesFile { FontId = selectedFont }, WriteOptions);
            using (var stream = new FileStream(temporaryPath, FileMode.Create, FileAccess.Write, FileShare.None))
            {
                stream.Write(bytes);
                stream.Flush(true);
            }
            File.Move(temporaryPath, _path, true);
        }
        catch
        {
            try
            {
                File.Delete(temporaryPath);
            }
            catch (IOException)
            {
            }
            throw;
        }
        FontId = selectedFont;
    }

    private sealed class PreferencesFile
    {
        public int Version { get; set; } = 1;
        public string FontId { get; set; } = BundledFont;
    }
}
