using System;
using System.Globalization;
using Microsoft.UI.Xaml.Data;

namespace GlideDock.Settings;

/// <summary>
/// Formats slider values for the live value labels next to each slider.
/// Using a binding (instead of code-behind text assignment) keeps the label in
/// sync during the whole drag gesture.
/// </summary>
public sealed class ValueLabelConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, string language)
    {
        var number = value switch
        {
            double d => d,
            float f => f,
            int i => i,
            _ => 0.0
        };
        return (parameter as string) switch
        {
            "px" => number.ToString("0", CultureInfo.InvariantCulture) + " px",
            "px_signed" => number.ToString("+0;-0;0", CultureInfo.InvariantCulture) + " px",
            "percent" => number.ToString("0", CultureInfo.InvariantCulture) + "%",
            "scale" => number.ToString("0.00", CultureInfo.InvariantCulture) + "×",
            "ms" => number.ToString("0", CultureInfo.InvariantCulture) + " ms",
            "one_decimal" => number.ToString("0.0", CultureInfo.InvariantCulture),
            _ => number.ToString("0.##", CultureInfo.InvariantCulture)
        };
    }

    public object ConvertBack(object value, Type targetType, object parameter, string language)
    {
        throw new NotSupportedException();
    }
}
