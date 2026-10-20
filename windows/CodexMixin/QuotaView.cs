using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Microsoft.Win32;
using Windows.Foundation;

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
    private static readonly Windows.UI.Color[] RingColors =
    [
        Windows.UI.Color.FromArgb(255, 0x23, 0x83, 0xE2),
        Windows.UI.Color.FromArgb(255, 0x2F, 0x9E, 0x72),
        Windows.UI.Color.FromArgb(255, 0xD9, 0x73, 0x0D),
        Windows.UI.Color.FromArgb(255, 0x90, 0x65, 0xB0),
    ];

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

    internal static Border StyleSwitch(QuotaStyle current, Action<QuotaStyle> changed)
    {
        var buttons = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 2 };
        foreach (var (style, label) in new[] { (QuotaStyle.Ring, "圆环"), (QuotaStyle.Bar, "条形") })
        {
            var button = new ToggleButton { Style = Themed("Segment"), Content = label, IsChecked = style == current, FontSize = 12, Padding = new Thickness(10, 2, 10, 2) };
            AutomationProperties.SetName(button, $"额度{label}展示");
            button.Click += (_, _) =>
            {
                foreach (var other in buttons.Children.Cast<ToggleButton>()) other.IsChecked = other == button;
                Store(style);
                changed(style);
            };
            buttons.Children.Add(button);
        }
        return new Border { Style = Themed("SegmentTrack"), Child = buttons, VerticalAlignment = VerticalAlignment.Center };
    }

    /// <summary>Renders the quota windows of one provider.</summary>
    internal static FrameworkElement Render(IReadOnlyList<JsonElement> rows, QuotaStyle style)
    {
        var entries = rows.Select(Parse).ToArray();
        if (style == QuotaStyle.Bar) return Bars(entries);
        var layout = new Grid { ColumnSpacing = 14 };
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        layout.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        layout.Children.Add(Rings(entries));
        var legend = new StackPanel { Spacing = 6, VerticalAlignment = VerticalAlignment.Center };
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
            "five_hour" => "5 小时", "weekly" => "每周", "monthly" => "每月", "balance" => "余额",
            _ => Json.OptionalText(row, "label") ?? Json.OptionalText(row, "quota_id") ?? "额度",
        };
        if (Json.OptionalText(row, "error") is { } error) return new Entry(label, "查询失败", null, error, null);
        double? ratio = limit is > 0 && used is { } consumed ? consumed / limit.Value
            : limit is > 0 && remaining is { } left ? Math.Max(0, (limit.Value - left) / limit.Value) : null;
        var value = used is { } amount && limit is { } maximum ? $"{amount:0.##} / {maximum:0.##} {currency}".TrimEnd()
            : remaining is { } balance ? $"余额 {balance:0.##} {currency}".TrimEnd()
            : used is { } spent ? $"已用 {spent:0.##} {currency}".TrimEnd() : "额度未知";
        var note = Json.OptionalText(row, "reset_at") is { } reset && DateTimeOffset.TryParse(reset, out var resetAt)
            ? $"{resetAt.ToLocalTime():MM-dd HH:mm} 重置"
            : !percent && remaining is { } rest && limit is > 0 ? $"剩余 {rest:0.##} {currency}".TrimEnd() : null;
        return new Entry(label, value, note, null, ratio);
    }

    private static double? Amount(JsonElement row, string field)
    {
        if (!row.TryGetProperty(field, out var value) || value.ValueKind == JsonValueKind.Null) return null;
        if (value.ValueKind != JsonValueKind.Number || !value.TryGetDouble(out var number) || !double.IsFinite(number) || number < 0)
            throw new InvalidOperationException($"CLI 返回了无效的 {field} 数值。");
        return number;
    }

    // Also colors usage chart series, so a provider keeps one hue per position.
    internal static Brush RingBrush(int index) => index < RingColors.Length
        ? new SolidColorBrush(RingColors[index])
        : (Brush)Microsoft.UI.Xaml.Application.Current.Resources["ControlStrongStrokeColorDisabledBrush"];

    private static Brush Resource(string key) => (Brush)Microsoft.UI.Xaml.Application.Current.Resources[key];

    private static bool Warning(Entry entry) => entry.Ratio >= WarningRatio;

    private static Canvas Rings(Entry[] entries)
    {
        var canvas = new Canvas { Width = RingDiameter, Height = RingDiameter, VerticalAlignment = VerticalAlignment.Center };
        AutomationProperties.SetAccessibilityView(canvas, AccessibilityView.Raw);
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
                    Size = new Size(radius, radius), IsLargeArc = angle > Math.PI, SweepDirection = SweepDirection.Clockwise,
                });
                var geometry = new PathGeometry();
                geometry.Figures.Add(figure);
                arc = new Microsoft.UI.Xaml.Shapes.Path { Data = geometry, StrokeStartLineCap = PenLineCap.Round, StrokeEndLineCap = PenLineCap.Round };
            }
            arc.Stroke = brush; arc.StrokeThickness = stroke;
            canvas.Children.Add(arc);
        }
        // The center shows the tightest window, the one that runs out first.
        if (entries.Where(entry => entry.Ratio != null).MaxBy(entry => entry.Ratio) is { Ratio: { } ratio } tightest)
        {
            var label = new TextBlock { Text = Percent(ratio), Foreground = Warning(tightest) ? Resource("SystemFillColorCriticalBrush") : Resource("TextFillColorPrimaryBrush"), FontSize = 18, FontWeight = Microsoft.UI.Text.FontWeights.Bold, Width = RingDiameter, TextAlignment = TextAlignment.Center };
            Canvas.SetTop(label, center - 13);
            canvas.Children.Add(label);
        }
        return canvas;
    }

    private static StackPanel LegendRow(Entry entry, int index)
    {
        var heading = new Grid { ColumnSpacing = 6 };
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        var swatch = new Ellipse { Width = 7, Height = 7, Fill = RingBrush(index), VerticalAlignment = VerticalAlignment.Center };
        heading.Children.Add(swatch);
        var label = new TextBlock { Text = entry.Label, FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis };
        Grid.SetColumn(label, 1); heading.Children.Add(label);
        var percent = new TextBlock { Text = entry.Ratio is { } ratio ? Percent(ratio) : "—", FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold };
        if (Warning(entry)) percent.Foreground = Resource("SystemFillColorCriticalBrush");
        Grid.SetColumn(percent, 2); heading.Children.Add(percent);
        var row = new StackPanel { Spacing = 1 };
        row.Children.Add(heading);
        row.Children.Add(ValueText(entry));
        return row;
    }

    private static StackPanel Bars(Entry[] entries)
    {
        var body = new StackPanel { Spacing = 12 };
        foreach (var entry in entries)
        {
            var heading = new Grid { ColumnSpacing = 8 };
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            heading.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            heading.Children.Add(new TextBlock { Text = entry.Label, FontSize = 13, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, TextTrimming = TextTrimming.CharacterEllipsis });
            var value = ValueText(entry);
            Grid.SetColumn(value, 1); heading.Children.Add(value);
            var row = new StackPanel { Spacing = 6 };
            row.Children.Add(heading);
            if (entry.Ratio is { } ratio) row.Children.Add(Bar(Math.Clamp(ratio, 0, 1), Resource(Warning(entry) ? "SystemFillColorCriticalBrush" : "AccentFillColorDefaultBrush")));
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

    // Star columns size the fill without waiting for a layout pass.
    private static Grid Bar(double fraction, Brush fill)
    {
        var bar = new Grid { Height = 8 };
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(Math.Max(fraction, 0.0001), GridUnitType.Star) });
        bar.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(Math.Max(1 - fraction, 0.0001), GridUnitType.Star) });
        var track = new Border { Style = Themed("BarTrack") };
        Grid.SetColumnSpan(track, 2);
        bar.Children.Add(track);
        if (fraction > 0) bar.Children.Add(new Border { Style = Themed("BarFill"), Background = fill });
        return bar;
    }

    private static string Percent(double ratio) => $"{Math.Round(ratio * 100):0}%";

    private static Style Themed(string key) => (Style)Microsoft.UI.Xaml.Application.Current.Resources[key];
}
