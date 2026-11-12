using System.Text.Json;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Automation.Peers;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;
using Microsoft.Win32;

namespace CodexMixin;

internal enum QuotaStyle { Ring, Bar }

// Renders CLI quota rows as Activity-style concentric rings or classic bars.
// The display choice is a GUI preference, so it lives in the shell's own
// registry key next to its window state.
internal static class QuotaView
{
    private const string PreferenceKey = @"Software\CodexMixin";
    private const string PreferenceName = "QuotaDisplayStyle";
    private const double RingDiameter = 88;
    // Rings share a fixed band so the hole keeps room for the percentage
    // label; more rings means thinner strokes rather than a smaller center.
    private const double RingStrokeMaximum = 9;
    private const double RingBand = 22;
    private const double RingGap = 2;
    // More than four rings stop being legible; extra windows stay legend-only.
    private const int MaximumRings = 4;
    private const double WarningRatio = 0.9;
    // One hue per ring so concentric windows stay distinguishable; red is
    // left out because it marks windows close to their limit.
    private static readonly Color[] RingColors =
    [
        Color.FromArgb(255, 0x23, 0x83, 0xE2),
        Color.FromArgb(255, 0x2F, 0x9E, 0x72),
        Color.FromArgb(255, 0xD9, 0x73, 0x0D),
        Color.FromArgb(255, 0x90, 0x65, 0xB0),
    ];

    private static readonly Brush[] RingBrushes = RingColors.Select(color => (Brush)new SolidColorBrush(color)).ToArray();

    private sealed record Entry(string Label, string Value, string? Note, string? Error, double? Ratio);

    internal static QuotaStyle Stored()
    {
        using var key = Registry.CurrentUser.OpenSubKey(PreferenceKey);
        return key?.GetValue(PreferenceName) as string == "bar" ? QuotaStyle.Bar : QuotaStyle.Ring;
    }

    private static void Store(QuotaStyle style)
    {
        using var key = Registry.CurrentUser.CreateSubKey(PreferenceKey, true);
        key.SetValue(PreferenceName, style == QuotaStyle.Bar ? "bar" : "ring");
    }

    internal static FrameworkElement StyleSwitch(QuotaStyle current, Action<QuotaStyle> changed)
    {
        var selector = new Stack { Orientation = Orientation.Horizontal, Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
        var group = $"quota-style-{Guid.NewGuid():N}";
        foreach (var (style, label) in new[] { (QuotaStyle.Ring, Localized.Get("圆环")), (QuotaStyle.Bar, Localized.Get("条形")) })
        {
            var option = new RadioButton
            {
                Content = label, GroupName = group, IsChecked = style == current,
                Style = (Style)Application.Current.Resources["DefaultToggleButtonStyle"],
                Padding = new Thickness(10, 3, 10, 3), MinHeight = 0, FontSize = 12
            };
            AutomationProperties.SetName(option, Localized.Format("额度{0}展示", label));
            option.Checked += (_, _) => { Store(style); changed(style); };
            selector.Children.Add(option);
        }
        return selector;
    }

    /// <summary>Renders the quota windows of one provider.</summary>
    internal static FrameworkElement Render(IReadOnlyList<JsonElement> rows, QuotaStyle style)
    {
        var entries = rows.Select(Parse).ToArray();
        if (style == QuotaStyle.Bar) return Bars(entries);
        var layout = new SpacedGrid { ColumnSpacing = 14 };
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        layout.Children.Add(Rings(entries));
        var legend = new Stack { Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
        for (var index = 0; index < entries.Length; index++) legend.Children.Add(LegendRow(entries[index], index));
        Grid.SetColumn(legend, 1); layout.Children.Add(legend);
        return layout;
    }

    private static Entry Parse(JsonElement row)
    {
        var used = Amount(row, "used") ?? Amount(row, "value");
        var limit = Amount(row, "limit");
        var remaining = Amount(row, "remaining");
        var percent = Json.OptionalText(row, "unit") == "percent";
        var currency = percent ? "" : Json.OptionalText(row, "currency") ?? "";
        var label = Json.OptionalText(row, "quota_id") switch
        {
            "five_hour" => Localized.Get("5 小时"),
            "weekly" => Localized.Get("每周"),
            "monthly" => Localized.Get("每月"),
            "balance" => Localized.Get("余额"),
            _ => Json.OptionalText(row, "label") ?? Json.OptionalText(row, "quota_id") ?? Localized.Get("额度"),
        };
        if (Json.OptionalText(row, "error") is { } error) return new Entry(label, Localized.Get("查询失败"), null, error, null);
        double? ratio = limit is > 0 && used is { } consumed ? consumed / limit.Value
            : limit is > 0 && remaining is { } left ? Math.Max(0, (limit.Value - left) / limit.Value) : null;
        var value = used is { } amount && limit is { } maximum ? $"{amount:0.##} / {maximum:0.##} {currency}".TrimEnd()
            : remaining is { } balance ? Localized.Format("余额 {0:0.##} {1}", balance, currency).TrimEnd()
            : used is { } spent ? Localized.Format("已用 {0:0.##} {1}", spent, currency).TrimEnd() : Localized.Get("额度未知");
        var note = Json.OptionalText(row, "reset_at") is { } reset && DateTimeOffset.TryParse(reset, out var resetAt)
            ? Localized.Format("{0:MM-dd HH:mm} 重置", resetAt.ToLocalTime())
            : !percent && remaining is { } rest && limit is > 0 ? Localized.Format("剩余 {0:0.##} {1}", rest, currency).TrimEnd() : null;
        return new Entry(label, value, note, null, ratio);
    }

    private static double? Amount(JsonElement row, string field)
    {
        if (!row.TryGetProperty(field, out var value) || value.ValueKind == JsonValueKind.Null) return null;
        if (value.ValueKind != JsonValueKind.Number || !value.TryGetDouble(out var number) || !double.IsFinite(number) || number < 0)
            throw new InvalidOperationException(Localized.Format("CLI 返回了无效的 {0} 数值。", field));
        return number;
    }

    // Also colors usage chart series, so a provider keeps one hue per position.
    internal static Brush RingBrush(int index) => index < RingBrushes.Length
        ? RingBrushes[index]
        : (Brush)Application.Current.Resources["ControlStrongStrokeColorDisabledBrush"];

    private static bool Warning(Entry entry) => entry.Ratio >= WarningRatio;

    private static Canvas Rings(Entry[] entries)
    {
        var canvas = new Canvas { Width = RingDiameter, Height = RingDiameter, VerticalAlignment = VerticalAlignment.Center };
        const double center = RingDiameter / 2;
        var shown = Math.Min(entries.Length, MaximumRings);
        var stroke = shown == 0 ? RingStrokeMaximum : Math.Min(RingStrokeMaximum, (RingBand - RingGap * (shown - 1)) / shown);
        for (var index = 0; index < shown; index++)
        {
            var entry = entries[index];
            var radius = center - stroke / 2 - index * (stroke + RingGap);
            var brush = RingBrush(index);
            var track = new Ellipse { Width = radius * 2, Height = radius * 2, Stroke = brush, StrokeThickness = stroke, Opacity = 0.18 };
            Canvas.SetLeft(track, center - radius); Canvas.SetTop(track, center - radius);
            canvas.Children.Add(track);
            var fill = Math.Clamp(entry.Ratio ?? 0, 0, 1);
            if (fill <= 0) continue;
            Shape arc;
            if (fill >= 0.999)
            {
                arc = new Ellipse { Width = radius * 2, Height = radius * 2 };
                Canvas.SetLeft(arc, center - radius); Canvas.SetTop(arc, center - radius);
            }
            else
            {
                // Clockwise from 12 o'clock, like the Activity rings.
                var angle = fill * 2 * Math.PI;
                var figure = new PathFigure { StartPoint = new Point(center, center - radius), IsClosed = false, IsFilled = false };
                figure.Segments.Add(new ArcSegment
                {
                    Point = new Point(center + radius * Math.Sin(angle), center - radius * Math.Cos(angle)),
                    Size = new Size(radius, radius),
                    IsLargeArc = angle > Math.PI,
                    SweepDirection = SweepDirection.Clockwise,
                });
                var geometry = new PathGeometry();
                geometry.Figures.Add(figure);
                arc = new System.Windows.Shapes.Path { Data = geometry, StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round };
            }
            arc.Stroke = brush; arc.StrokeThickness = stroke;
            canvas.Children.Add(arc);
        }
        // The center shows the tightest window, the one that runs out first.
        if (entries.Where(entry => entry.Ratio != null).MaxBy(entry => entry.Ratio) is { Ratio: { } ratio } tightest)
        {
            var label = new TextBlock { Text = Percent(ratio), Style = Themed(Warning(tightest) ? "QuotaPercentCritical" : "QuotaPercent"), FontSize = 18, FontWeight = FontWeights.Bold, Width = RingDiameter, TextAlignment = TextAlignment.Center };
            AutomationProperties.SetAutomationId(label, "quota-ring-percent");
            Canvas.SetTop(label, center - 13);
            canvas.Children.Add(label);
        }
        return canvas;
    }

    private static Stack LegendRow(Entry entry, int index)
    {
        var heading = new SpacedGrid { ColumnSpacing = 6 };
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var swatch = new Ellipse { Width = 7, Height = 7, Fill = RingBrush(index), VerticalAlignment = VerticalAlignment.Center };
        heading.Children.Add(swatch);
        var label = new TextBlock { Text = entry.Label, FontSize = 12, FontWeight = FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis };
        Grid.SetColumn(label, 1); heading.Children.Add(label);
        var percent = new TextBlock { Text = entry.Ratio is { } ratio ? Percent(ratio) : "—", FontSize = 12, FontWeight = FontWeights.SemiBold };
        percent.Style = Themed(Warning(entry) ? "QuotaPercentCritical" : "QuotaPercent");
        Grid.SetColumn(percent, 2); heading.Children.Add(percent);
        var row = new Stack { Spacing = 1 };
        row.Children.Add(heading);
        row.Children.Add(ValueText(entry));
        return row;
    }

    private static Stack Bars(Entry[] entries)
    {
        var body = new Stack { Spacing = 12 };
        foreach (var entry in entries)
        {
            var heading = new SpacedGrid { ColumnSpacing = 8 };
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            heading.Children.Add(new TextBlock { Text = entry.Label, FontSize = 13, FontWeight = FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis });
            var value = ValueText(entry);
            Grid.SetColumn(value, 1); heading.Children.Add(value);
            var row = new Stack { Spacing = 6 };
            row.Children.Add(heading);
            if (entry.Ratio is { } ratio) row.Children.Add(Bar(entry, ratio));
            body.Children.Add(row);
        }
        return body;
    }

    private static TextBlock ValueText(Entry entry)
    {
        var text = entry.Error is { } error ? $"{entry.Value}：{error}" : entry.Note is { } note ? $"{entry.Value} · {note}" : entry.Value;
        var block = new TextBlock { Text = text, Style = Themed(entry.Error == null ? "Mono" : "CautionCaption"), MaxWidth = 240, TextTrimming = TextTrimming.CharacterEllipsis };
        ToolTipService.SetToolTip(block, text);
        return block;
    }

    private static ProgressBar Bar(Entry entry, double ratio)
    {
        var bar = new ProgressBar
        {
            Minimum = 0,
            Maximum = 100,
            Value = Math.Clamp(ratio, 0, 1) * 100,
            IsIndeterminate = false,
            HorizontalAlignment = HorizontalAlignment.Stretch
        };
        if (Warning(entry)) bar.SetResourceReference(Control.ForegroundProperty, "SystemFillColorCriticalBrush");
        AutomationProperties.SetName(bar, Localized.Format("{0}已用比例", entry.Label));
        return bar;
    }

    private static string Percent(double ratio) => $"{Math.Round(ratio * 100):0}%";

    private static Style Themed(string key) => (Style)Application.Current.Resources[key];
}
