using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace GlideDock.Settings;

/// <summary>
/// Reads and writes Glide-Dock's JSON configuration while preserving item data and
/// unknown fields written by other components.
/// </summary>
public sealed class SettingsDocument
{
    private static readonly JsonSerializerOptions WriteOptions = new()
    {
        WriteIndented = true
    };

    public string Path { get; }

    private JsonObject _root;

    private SettingsDocument(string path, JsonObject root)
    {
        Path = path;
        _root = root;
        EnsureCoreFields();
    }

    public static SettingsDocument Load(string path)
    {
        if (!File.Exists(path))
        {
            return new SettingsDocument(path, DefaultRoot());
        }

        try
        {
            var text = File.ReadAllText(path, Encoding.UTF8);
            if (JsonNode.Parse(text) is not JsonObject root)
            {
                throw new InvalidDataException("配置文件的根节点不是 JSON 对象。");
            }
            return new SettingsDocument(path, root);
        }
        catch (JsonException ex)
        {
            throw new InvalidDataException("配置文件不是合法 JSON：" + ex.Message, ex);
        }
    }

    public void Reload()
    {
        var loaded = Load(Path);
        _root = loaded._root;
        EnsureCoreFields();
    }

    private static JsonObject DefaultRoot() => new()
    {
        ["version"] = 3,
        ["appearance"] = new JsonObject
        {
            ["icon_size"] = 48.0,
            ["spacing"] = 18.0,
            ["padding"] = 14.0,
            ["corner_radius"] = 20.0,
            ["opacity"] = 0.78,
            ["theme"] = "system",
            ["accent"] = "blue",
            ["position"] = "bottom",
            ["layer"] = "desktop",
            ["edge_margin"] = 10.0,
            ["frosted_glass"] = true
        },
        ["behavior"] = new JsonObject
        {
            ["magnification"] = true,
            ["auto_hide"] = false,
            ["auto_hide_delay_ms"] = 450,
            ["launch_animation"] = true,
            ["show_tooltips"] = true,
            ["show_running_indicator"] = true
        },
        ["motion"] = DefaultMotion(),
        ["monitor"] = new JsonObject
        {
            ["display_id"] = "",
            ["offset"] = 0.0
        },
        ["items"] = new JsonArray(),
        ["next_item_id"] = 1
    };

    private static JsonObject DefaultMotion() => new()
    {
        ["enabled"] = true,
        ["respect_reduced_motion"] = true,
        ["hover_preset"] = "glide_zoom",
        ["launch_preset"] = "glide_launch",
        ["max_scale"] = 1.35,
        ["range_slots"] = 1.7,
        ["hover_ms"] = 180,
        ["elasticity"] = 0.75
    };

    private void EnsureCoreFields()
    {
        var version = 1;
        if (_root["version"] is JsonValue versionValue
            && versionValue.TryGetValue<int>(out var parsedVersion))
        {
            version = parsedVersion;
        }
        if (version < 3)
        {
            _root["version"] = 3;
        }
        if (_root["items"] is not JsonArray)
        {
            _root["items"] = new JsonArray();
        }
        if (_root["next_item_id"] is null)
        {
            _root["next_item_id"] = 1;
        }
        EnsureObject("appearance");
        EnsureObject("behavior");
        EnsureMotion();
        EnsureObject("monitor");
    }

    private JsonObject EnsureMotion()
    {
        if (_root["motion"] is JsonObject existing)
        {
            foreach (var (key, value) in DefaultMotion())
            {
                if (existing[key] is null)
                {
                    existing[key] = value?.DeepClone();
                }
            }
            return existing;
        }

        var motion = DefaultMotion();
        // One-time compatibility with v1 files: keep the user's magnification
        // and spring feel instead of resetting it to the new defaults.
        var behavior = EnsureObject("behavior");
        if (behavior["magnification_factor"] is JsonValue factor
            && factor.TryGetValue<double>(out var factorValue))
        {
            motion["max_scale"] = Math.Clamp(factorValue, 1.05, 3.5);
        }
        if (behavior["magnification_range"] is JsonValue range
            && range.TryGetValue<double>(out var rangeValue))
        {
            motion["range_slots"] = Math.Clamp(rangeValue, 0.5, 8.0);
        }
        var stiffness = 560.0;
        var damping = 0.72;
        if (behavior["stiffness"] is JsonValue stiffnessValue
            && stiffnessValue.TryGetValue<double>(out var parsedStiffness))
        {
            stiffness = parsedStiffness;
        }
        if (behavior["damping"] is JsonValue dampingValue
            && dampingValue.TryGetValue<double>(out var parsedDamping))
        {
            damping = parsedDamping;
        }
        var zeta = Math.Clamp(damping, 0.35, 1.0);
        var omega = Math.Sqrt(Math.Max(1.0, stiffness));
        var hoverSeconds = Math.Clamp(3.912 / (zeta * omega), 0.08, 0.4);
        motion["hover_ms"] = Math.Clamp((int)Math.Round(hoverSeconds * 1000.0), 80, 400);
        motion["elasticity"] = Math.Round(Math.Clamp((1.0 - zeta) / 0.4, 0.0, 1.0), 2);
        _root["motion"] = motion;
        return motion;
    }

    private JsonObject EnsureObject(string name)
    {
        if (_root[name] is JsonObject existing)
        {
            return existing;
        }
        var created = new JsonObject();
        _root[name] = created;
        return created;
    }

    private JsonObject Section(string section) => EnsureObject(section);

    public float GetFloat(string section, string key, float fallback)
    {
        if (Section(section)[key] is not JsonValue value)
        {
            return fallback;
        }
        try
        {
            return (float)value.GetValue<double>();
        }
        catch
        {
            return fallback;
        }
    }

    public void SetFloat(string section, string key, float value)
    {
        Section(section)[key] = value;
    }

    public string GetString(string section, string key, string fallback)
    {
        if (Section(section)[key] is not JsonValue value)
        {
            return fallback;
        }
        try
        {
            return value.GetValue<string>() ?? fallback;
        }
        catch
        {
            return fallback;
        }
    }

    public void SetString(string section, string key, string value)
    {
        Section(section)[key] = value;
    }

    public bool GetBool(string section, string key, bool fallback)
    {
        if (Section(section)[key] is not JsonValue value)
        {
            return fallback;
        }
        try
        {
            return value.GetValue<bool>();
        }
        catch
        {
            return fallback;
        }
    }

    public int GetInt(string section, string key, int fallback)
    {
        if (Section(section)[key] is not JsonValue value)
        {
            return fallback;
        }
        try
        {
            return value.GetValue<int>();
        }
        catch
        {
            return fallback;
        }
    }

    public void SetInt(string section, string key, int value)
    {
        Section(section)[key] = value;
    }

    public void SetBool(string section, string key, bool value)
    {
        Section(section)[key] = value;
    }

    public string Theme
    {
        get => GetString("appearance", "theme", "system");
        set => SetString("appearance", "theme", value);
    }

    public string Accent
    {
        get => GetString("appearance", "accent", "blue");
        set => SetString("appearance", "accent", value);
    }

    public string Layer
    {
        get => GetString("appearance", "layer", "desktop");
        set => SetString("appearance", "layer", value);
    }

    public float IconSize
    {
        get => GetFloat("appearance", "icon_size", 48);
        set => SetFloat("appearance", "icon_size", value);
    }

    public float Spacing
    {
        get => GetFloat("appearance", "spacing", 18);
        set => SetFloat("appearance", "spacing", value);
    }

    public float Padding
    {
        get => GetFloat("appearance", "padding", 14);
        set => SetFloat("appearance", "padding", value);
    }

    public float CornerRadius
    {
        get => GetFloat("appearance", "corner_radius", 20);
        set => SetFloat("appearance", "corner_radius", value);
    }

    public float Opacity
    {
        get => GetFloat("appearance", "opacity", 0.78f);
        set => SetFloat("appearance", "opacity", value);
    }

    public float EdgeMargin
    {
        get => GetFloat("appearance", "edge_margin", 10);
        set => SetFloat("appearance", "edge_margin", value);
    }

    public float MonitorOffset
    {
        get => GetFloat("monitor", "offset", 0);
        set => SetFloat("monitor", "offset", value);
    }

    public bool Magnification
    {
        get => GetBool("behavior", "magnification", true);
        set => SetBool("behavior", "magnification", value);
    }

    public bool ShowTooltips
    {
        get => GetBool("behavior", "show_tooltips", true);
        set => SetBool("behavior", "show_tooltips", value);
    }

    public bool LaunchAnimation
    {
        get => GetBool("behavior", "launch_animation", true);
        set => SetBool("behavior", "launch_animation", value);
    }

    public bool MotionEnabled
    {
        get => GetBool("motion", "enabled", true);
        set => SetBool("motion", "enabled", value);
    }

    public bool RespectReducedMotion
    {
        get => GetBool("motion", "respect_reduced_motion", true);
        set => SetBool("motion", "respect_reduced_motion", value);
    }

    public string HoverPreset
    {
        get => GetString("motion", "hover_preset", "glide_zoom");
        set => SetString("motion", "hover_preset", value);
    }

    public string LaunchPreset
    {
        get => GetString("motion", "launch_preset", "glide_launch");
        set => SetString("motion", "launch_preset", value);
    }

    public float MaxScale
    {
        get => GetFloat("motion", "max_scale", 1.45f);
        set => SetFloat("motion", "max_scale", value);
    }

    public float RangeSlots
    {
        get => GetFloat("motion", "range_slots", 2.6f);
        set => SetFloat("motion", "range_slots", value);
    }

    public int HoverMilliseconds
    {
        get => GetInt("motion", "hover_ms", 180);
        set => SetInt("motion", "hover_ms", value);
    }

    public float Elasticity
    {
        get => GetFloat("motion", "elasticity", 0.75f);
        set => SetFloat("motion", "elasticity", value);
    }

    /// <summary>
    /// Serialises only the motion section, used by the "copy animation config"
    /// button. The full document is never copied.
    /// </summary>
    public string MotionJson()
    {
        var motion = EnsureMotion();
        var copy = new JsonObject();
        foreach (var key in new[]
        {
            "enabled", "respect_reduced_motion", "hover_preset", "launch_preset",
            "max_scale", "range_slots", "hover_ms", "elasticity"
        })
        {
            if (motion[key] is JsonNode node)
            {
                copy[key] = node.DeepClone();
            }
        }
        return copy.ToJsonString(WriteOptions) + "\n";
    }

    public bool ShowRunningIndicator
    {
        get => GetBool("behavior", "show_running_indicator", true);
        set => SetBool("behavior", "show_running_indicator", value);
    }

    public void ResetDefaults()
    {
        var defaults = DefaultRoot();
        _root["appearance"] = defaults["appearance"]!.DeepClone();
        _root["behavior"] = defaults["behavior"]!.DeepClone();
        _root["motion"] = defaults["motion"]!.DeepClone();
        _root["monitor"] = defaults["monitor"]!.DeepClone();
        _root["version"] = 3;
        EnsureCoreFields();
    }

    public string ToJson() => _root.ToJsonString(WriteOptions) + "\n";

    public void Save()
    {
        var directory = System.IO.Path.GetDirectoryName(Path);
        if (!string.IsNullOrEmpty(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var temp = Path + ".settings.tmp";
        using (var stream = new FileStream(temp, FileMode.Create, FileAccess.Write, FileShare.None))
        {
            var bytes = new UTF8Encoding(false).GetBytes(ToJson());
            stream.Write(bytes, 0, bytes.Length);
            stream.Flush(true);
        }
        File.Move(temp, Path, true);
    }

    public bool Send(IntPtr owner, IntPtr sender)
    {
        if (owner == IntPtr.Zero || sender == IntPtr.Zero)
        {
            return false;
        }
        return NativeMethods.SendConfig(owner, sender, Encoding.UTF8.GetBytes(ToJson()));
    }
}
