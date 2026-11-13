using System.Globalization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Data;
using System.Windows.Documents;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Threading;

namespace CodexMixin;

// WPF lacks several WinUI building blocks; these stay deliberately small and lean on the Fluent theme.

// StackPanel with Spacing and Padding; collapsed children take no gap.
internal class Stack : Panel
{
    private Orientation orientation = Orientation.Vertical;
    private double spacing;
    private Thickness padding;

    public Orientation Orientation { get => orientation; set { orientation = value; InvalidateMeasure(); } }
    public double Spacing { get => spacing; set { spacing = value; InvalidateMeasure(); } }
    public Thickness Padding { get => padding; set { padding = value; InvalidateMeasure(); } }

    private bool Vertical => orientation == Orientation.Vertical;

    protected override Size MeasureOverride(Size availableSize)
    {
        var inner = new Size(
            Math.Max(0, availableSize.Width - padding.Left - padding.Right),
            Math.Max(0, availableSize.Height - padding.Top - padding.Bottom));
        var slot = Vertical ? new Size(inner.Width, double.PositiveInfinity) : new Size(double.PositiveInfinity, inner.Height);
        double along = 0, across = 0;
        var count = 0;
        foreach (UIElement child in InternalChildren)
        {
            child.Measure(slot);
            if (child.Visibility == Visibility.Collapsed) continue;
            var size = child.DesiredSize;
            along += Vertical ? size.Height : size.Width;
            across = Math.Max(across, Vertical ? size.Width : size.Height);
            count++;
        }
        along += Math.Max(0, count - 1) * spacing;
        return Vertical
            ? new Size(across + padding.Left + padding.Right, along + padding.Top + padding.Bottom)
            : new Size(along + padding.Left + padding.Right, across + padding.Top + padding.Bottom);
    }

    protected override Size ArrangeOverride(Size finalSize)
    {
        var offset = Vertical ? padding.Top : padding.Left;
        var across = Vertical
            ? Math.Max(0, finalSize.Width - padding.Left - padding.Right)
            : Math.Max(0, finalSize.Height - padding.Top - padding.Bottom);
        foreach (UIElement child in InternalChildren)
        {
            if (child.Visibility == Visibility.Collapsed) continue;
            var size = child.DesiredSize;
            child.Arrange(Vertical
                ? new Rect(padding.Left, offset, across, size.Height)
                : new Rect(offset, padding.Top, size.Width, across));
            offset += (Vertical ? size.Height : size.Width) + spacing;
        }
        return finalSize;
    }
}

// Grid with WinUI-style ColumnSpacing/RowSpacing, applied as leading margins on cells after the first.
internal class SpacedGrid : Grid
{
    private static readonly DependencyProperty AppliedProperty =
        DependencyProperty.RegisterAttached("Applied", typeof(Thickness), typeof(SpacedGrid), new PropertyMetadata(default(Thickness)));

    public double ColumnSpacing { get; set; }
    public double RowSpacing { get; set; }

    protected override Size MeasureOverride(Size constraint)
    {
        foreach (var child in InternalChildren.OfType<FrameworkElement>())
        {
            var gap = new Thickness(GetColumn(child) > 0 ? ColumnSpacing : 0, GetRow(child) > 0 ? RowSpacing : 0, 0, 0);
            var applied = (Thickness)child.GetValue(AppliedProperty);
            if (gap == applied) continue;
            var margin = child.Margin;
            child.Margin = new Thickness(margin.Left - applied.Left + gap.Left, margin.Top - applied.Top + gap.Top, margin.Right, margin.Bottom);
            child.SetValue(AppliedProperty, gap);
        }
        return base.MeasureOverride(constraint);
    }
}

// Segoe Fluent Icons glyph.
internal sealed class FontIcon : TextBlock
{
    public FontIcon()
    {
        SetResourceReference(FontFamilyProperty, "SymbolThemeFontFamily");
        FontSize = 16;
        VerticalAlignment = VerticalAlignment.Center;
        TextAlignment = TextAlignment.Center;
    }

    public string Glyph { get => Text; set => Text = value; }
}

internal sealed class ToggleSwitch : ToggleButton
{
    public event RoutedEventHandler? Toggled;

    public bool IsOn { get => IsChecked == true; set => IsChecked = value; }

    protected override void OnChecked(RoutedEventArgs e) { base.OnChecked(e); Toggled?.Invoke(this, e); }

    protected override void OnUnchecked(RoutedEventArgs e) { base.OnUnchecked(e); Toggled?.Invoke(this, e); }
}

// Numeric TextBox: Value is NaN when the text is empty or invalid; arrow keys step by one.
internal sealed class NumberBox : TextBox
{
    public double Minimum { get; set; } = double.MinValue;
    public double Maximum { get; set; } = double.MaxValue;
    public event EventHandler? ValueChanged;

    public double Value
    {
        get => double.TryParse(Text, NumberStyles.Float, CultureInfo.CurrentCulture, out var value) ? value : double.NaN;
        set => Text = double.IsFinite(value) ? value.ToString(CultureInfo.CurrentCulture) : "";
    }

    protected override void OnTextChanged(TextChangedEventArgs e) { base.OnTextChanged(e); ValueChanged?.Invoke(this, EventArgs.Empty); }

    protected override void OnPreviewKeyDown(KeyEventArgs e)
    {
        base.OnPreviewKeyDown(e);
        if (e.Key is not (Key.Up or Key.Down)) return;
        var current = double.IsFinite(Value) ? Value : Math.Max(0, Minimum);
        Value = Math.Clamp(current + (e.Key == Key.Up ? 1 : -1), Minimum, Maximum);
        CaretIndex = Text.Length;
        e.Handled = true;
    }

    protected override void OnLostKeyboardFocus(KeyboardFocusChangedEventArgs e)
    {
        base.OnLostKeyboardFocus(e);
        if (double.IsFinite(Value) && (Value < Minimum || Value > Maximum)) Value = Math.Clamp(Value, Minimum, Maximum);
    }
}

// 24-hour picker built from two native ComboBoxes.
internal sealed class TimePicker : Stack
{
    private readonly ComboBox hours = new() { MinWidth = 64 };
    private readonly ComboBox minutes = new() { MinWidth = 64 };
    private bool updating;
    public event EventHandler? TimeChanged;

    public TimePicker()
    {
        Orientation = Orientation.Horizontal;
        Spacing = 4;
        for (var hour = 0; hour < 24; hour++) hours.Items.Add($"{hour:00}");
        for (var minute = 0; minute < 60; minute++) minutes.Items.Add($"{minute:00}");
        AutomationProperties.SetName(hours, Localized.Get("小时"));
        AutomationProperties.SetName(minutes, Localized.Get("分钟"));
        hours.SelectionChanged += (_, _) => { if (!updating) TimeChanged?.Invoke(this, EventArgs.Empty); };
        minutes.SelectionChanged += (_, _) => { if (!updating) TimeChanged?.Invoke(this, EventArgs.Empty); };
        Children.Add(hours);
        Children.Add(new TextBlock { Text = ":", VerticalAlignment = VerticalAlignment.Center });
        Children.Add(minutes);
        Time = TimeSpan.Zero;
    }

    public TimeSpan Time
    {
        get => new(Math.Max(0, hours.SelectedIndex), Math.Max(0, minutes.SelectedIndex), 0);
        set { updating = true; hours.SelectedIndex = value.Hours; minutes.SelectedIndex = value.Minutes; updating = false; }
    }
}

// Header above a form control. The header text lives on the control (Field.Header) and the row follows its Visibility.
internal sealed class Field : Stack
{
    public static readonly DependencyProperty HeaderProperty =
        DependencyProperty.RegisterAttached("Header", typeof(string), typeof(Field), new PropertyMetadata(null));

    public static string? GetHeader(DependencyObject element) => (string?)element.GetValue(HeaderProperty);
    public static void SetHeader(DependencyObject element, string? value) => element.SetValue(HeaderProperty, value);

    internal FrameworkElement Control { get; }

    private Field(FrameworkElement control)
    {
        Control = control;
        Spacing = 6;
        var label = new HeaderText { TextWrapping = TextWrapping.Wrap };
        var header = new Binding { Source = control, Path = new PropertyPath(HeaderProperty) };
        label.SetBinding(TextBlock.TextProperty, header);
        // Like a WinUI Header, the label names the control instead of being a separate automation element.
        if (BindingOperations.GetBinding(control, AutomationProperties.NameProperty) == null && string.IsNullOrEmpty(AutomationProperties.GetName(control)))
            control.SetBinding(AutomationProperties.NameProperty, header);
        SetBinding(VisibilityProperty, new Binding(nameof(Visibility)) { Source = control });
        Children.Add(label);
        Children.Add(control);
    }

    private sealed class HeaderText : TextBlock
    {
        protected override System.Windows.Automation.Peers.AutomationPeer OnCreateAutomationPeer() => null!;
    }

    internal static T Labeled<T>(T control, string header) where T : FrameworkElement
    {
        SetHeader(control, header);
        return control;
    }

    // Wraps controls that carry a header; others pass through unchanged.
    internal static FrameworkElement Wrap(FrameworkElement control) => GetHeader(control) == null ? control : new Field(control);
}

internal static class Placeholder
{
    internal static void Set(TextBox box, string text)
    {
        AutomationProperties.SetHelpText(box, text);
        Hint? hint = null;
        box.Loaded += (_, _) =>
        {
            if (hint != null || AdornerLayer.GetAdornerLayer(box) is not { } layer) return;
            hint = new Hint(box, text) { IsHitTestVisible = false };
            layer.Add(hint);
        };
        box.TextChanged += (_, _) => hint?.InvalidateVisual();
        box.IsVisibleChanged += (_, _) => hint?.InvalidateVisual();
    }

    private sealed class Hint(TextBox box, string text) : Adorner(box)
    {
        protected override void OnRender(DrawingContext context)
        {
            if (box.Text.Length > 0 || !box.IsVisible) return;
            var brush = box.TryFindResource("TextControlPlaceholderForeground") as Brush ?? Brushes.Gray;
            var hint = new FormattedText(text, CultureInfo.CurrentUICulture, box.FlowDirection,
                new Typeface(box.FontFamily, box.FontStyle, box.FontWeight, box.FontStretch), box.FontSize, brush,
                VisualTreeHelper.GetDpi(this).PixelsPerDip)
            { MaxTextWidth = Math.Max(1, box.ActualWidth - box.Padding.Left - box.Padding.Right - 4), MaxLineCount = 1, Trimming = TextTrimming.CharacterEllipsis };
            context.DrawText(hint, new Point(box.BorderThickness.Left + box.Padding.Left + 2, (box.ActualHeight - hint.Height) / 2));
        }
    }
}

internal static class Menus
{
    // Opens the menu under the button on click, like WinUI Button.Flyout.
    internal static void Attach(ButtonBase button, ContextMenu menu, PlacementMode placement = PlacementMode.Bottom)
    {
        menu.PlacementTarget = button;
        menu.Placement = placement;
        button.Click += (_, _) => menu.IsOpen = true;
    }
}

internal enum InfoBarSeverity { Informational, Success, Warning, Error }

internal sealed class InfoBar : Border
{
    private readonly FontIcon icon = new() { Margin = new Thickness(0, 1, 12, 0), VerticalAlignment = VerticalAlignment.Top };
    private readonly Run title = new() { FontWeight = FontWeights.SemiBold };
    private readonly Run message = new();
    private readonly Button close = new() { Content = new FontIcon { Glyph = "\uE711", FontSize = 12 }, Padding = new Thickness(8), VerticalAlignment = VerticalAlignment.Top };

    public InfoBar()
    {
        CornerRadius = new CornerRadius(4);
        BorderThickness = new Thickness(1);
        Padding = new Thickness(14, 10, 4, 10);
        Visibility = Visibility.Collapsed;
        SetResourceReference(BorderBrushProperty, "CardStrokeColorDefaultBrush");
        close.SetResourceReference(StyleProperty, "FlatButton");
        AutomationProperties.SetName(close, Localized.Get("关闭"));
        close.Click += (_, _) => IsOpen = false;
        var text = new TextBlock { TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center };
        text.Inlines.Add(title);
        text.Inlines.Add(new Run("  "));
        text.Inlines.Add(message);
        AutomationProperties.SetLiveSetting(text, AutomationLiveSetting.Assertive);
        var layout = new DockPanel();
        DockPanel.SetDock(icon, Dock.Left);
        DockPanel.SetDock(close, Dock.Right);
        layout.Children.Add(icon);
        layout.Children.Add(close);
        layout.Children.Add(text);
        Child = layout;
        Severity = InfoBarSeverity.Informational;
    }

    public string Title { get => title.Text; set => title.Text = value; }
    public string Message { get => message.Text; set => message.Text = value; }
    public bool IsOpen { get => Visibility == Visibility.Visible; set => Visibility = value ? Visibility.Visible : Visibility.Collapsed; }
    public bool IsClosable { get => close.Visibility == Visibility.Visible; set => close.Visibility = value ? Visibility.Visible : Visibility.Collapsed; }

    public InfoBarSeverity Severity
    {
        set
        {
            var (name, glyph) = value switch
            {
                InfoBarSeverity.Success => ("Success", "\uE73E"),
                InfoBarSeverity.Warning => ("Caution", "\uE7BA"),
                InfoBarSeverity.Error => ("Critical", "\uEA39"),
                _ => ("Attention", "\uE946"),
            };
            SetResourceReference(BackgroundProperty, $"SystemFillColor{name}BackgroundBrush");
            icon.SetResourceReference(TextBlock.ForegroundProperty, $"SystemFillColor{name}Brush");
            icon.Glyph = glyph;
        }
    }
}

// Indeterminate spinner; animates only while active and visible.
internal sealed class ProgressRing : FrameworkElement
{
    public static readonly DependencyProperty ForegroundProperty = DependencyProperty.Register(
        nameof(Foreground), typeof(Brush), typeof(ProgressRing), new FrameworkPropertyMetadata(null, FrameworkPropertyMetadataOptions.AffectsRender));

    private readonly RotateTransform spin = new();
    private bool active;

    public ProgressRing()
    {
        RenderTransform = spin;
        RenderTransformOrigin = new Point(0.5, 0.5);
        SetResourceReference(ForegroundProperty, "AccentFillColorDefaultBrush");
        IsVisibleChanged += (_, _) => Animate();
    }

    public Brush? Foreground { get => (Brush?)GetValue(ForegroundProperty); set => SetValue(ForegroundProperty, value); }
    public bool IsActive { get => active; set { active = value; Animate(); } }

    private void Animate() => spin.BeginAnimation(RotateTransform.AngleProperty, active && IsVisible
        ? new DoubleAnimation(0, 360, TimeSpan.FromSeconds(1)) { RepeatBehavior = RepeatBehavior.Forever }
        : null);

    protected override void OnRender(DrawingContext context)
    {
        var thickness = Math.Max(1.5, Math.Min(ActualWidth, ActualHeight) / 8);
        var radius = Math.Min(ActualWidth, ActualHeight) / 2 - thickness / 2;
        if (radius <= 0) return;
        var center = new Point(ActualWidth / 2, ActualHeight / 2);
        var arc = new StreamGeometry();
        using (var path = arc.Open())
        {
            path.BeginFigure(new Point(center.X, center.Y - radius), false, false);
            path.ArcTo(new Point(center.X - radius, center.Y), new Size(radius, radius), 0, true, SweepDirection.Clockwise, true, false);
        }
        context.DrawGeometry(null, new Pen(Foreground ?? Brushes.Gray, thickness) { StartLineCap = PenLineCap.Round, EndLineCap = PenLineCap.Round }, arc);
    }
}

// ListView that hands each realized container to the page, like WinUI ContainerContentChanging.
internal sealed class VirtualList : ListView
{
    internal event Action<ListViewItem, object>? Prepared;
    internal event Action<ListViewItem, object>? Cleared;

    public VirtualList()
    {
        VirtualizingPanel.SetIsVirtualizing(this, true);
        VirtualizingPanel.SetVirtualizationMode(this, VirtualizationMode.Recycling);
        VirtualizingPanel.SetScrollUnit(this, ScrollUnit.Pixel);
        ScrollViewer.SetHorizontalScrollBarVisibility(this, ScrollBarVisibility.Disabled);
        // Lists sit inside cards and panes, so the default ListView frame would draw a second box.
        BorderThickness = new Thickness(0);
        Background = Brushes.Transparent;
    }

    protected override void PrepareContainerForItemOverride(DependencyObject element, object item)
    {
        base.PrepareContainerForItemOverride(element, item);
        Prepared?.Invoke((ListViewItem)element, item);
    }

    protected override void ClearContainerForItemOverride(DependencyObject element, object item)
    {
        Cleared?.Invoke((ListViewItem)element, item);
        base.ClearContainerForItemOverride(element, item);
    }
}

internal enum ContentDialogResult { None, Primary, Secondary }

internal enum ContentDialogButton { None, Primary, Secondary, Close }

// Modal Fluent window with the WinUI ContentDialog button contract (automation ids PrimaryButton/SecondaryButton/CloseButton).
internal sealed class ContentDialog
{
    public string IconGlyph { get; set; } = "\uE713";
    public string Title { get; set; } = "";
    public object? Content { get; set; }
    public string? PrimaryButtonText { get; set; }
    public string? SecondaryButtonText { get; set; }
    public string? CloseButtonText { get; set; }
    public ContentDialogButton DefaultButton { get; set; }
    public Window? Owner { get; set; }

    public async Task<ContentDialogResult> ShowAsync()
    {
        // Let the click, toggle or UI Automation call that opened the dialog return before the modal loop starts.
        await Dispatcher.Yield(DispatcherPriority.Background);
        var result = ContentDialogResult.None;
        var window = new Window
        {
            Title = Title,
            SizeToContent = SizeToContent.WidthAndHeight,
            ResizeMode = ResizeMode.NoResize,
            ShowInTaskbar = false,
            MinWidth = 360,
            MaxWidth = Math.Min(680, SystemParameters.WorkArea.Width - 32),
            MaxHeight = Math.Max(240, SystemParameters.WorkArea.Height - 48),
        };
        if (Owner is { IsVisible: true } owner)
        {
            window.Owner = owner;
            window.WindowStartupLocation = WindowStartupLocation.CenterOwner;
            window.Topmost = owner.Topmost;
        }
        else window.WindowStartupLocation = WindowStartupLocation.CenterScreen;

        var buttons = new Stack { Orientation = Orientation.Horizontal, Spacing = 8, HorizontalAlignment = HorizontalAlignment.Right };
        void Add(string? text, string id, ContentDialogButton kind, ContentDialogResult value)
        {
            if (string.IsNullOrEmpty(text)) return;
            var button = new Button { Content = text, MinWidth = 96, IsDefault = DefaultButton == kind, IsCancel = kind == ContentDialogButton.Close };
            if (DefaultButton == kind) button.SetResourceReference(FrameworkElement.StyleProperty, "AccentButtonStyle");
            AutomationProperties.SetAutomationId(button, id);
            button.Click += (_, _) => { result = value; window.Close(); };
            buttons.Children.Add(button);
        }
        Add(PrimaryButtonText, "PrimaryButton", ContentDialogButton.Primary, ContentDialogResult.Primary);
        Add(SecondaryButtonText, "SecondaryButton", ContentDialogButton.Secondary, ContentDialogResult.Secondary);
        Add(CloseButtonText, "CloseButton", ContentDialogButton.Close, ContentDialogResult.None);

        var body = new Stack { Spacing = 16, Padding = new Thickness(24, 20, 24, 24) };
        var heading = new TextBlock { Text = Title, TextWrapping = TextWrapping.Wrap };
        heading.SetResourceReference(FrameworkElement.StyleProperty, "SubtitleTextBlockStyle");
        var headingRow = new DockPanel();
        var icon = new FontIcon { Glyph = IconGlyph, FontSize = 24, Width = 32, Margin = new Thickness(0, 0, 12, 0), VerticalAlignment = VerticalAlignment.Center };
        icon.SetResourceReference(Control.ForegroundProperty, "AccentTextFillColorPrimaryBrush");
        DockPanel.SetDock(icon, Dock.Left); headingRow.Children.Add(icon); headingRow.Children.Add(heading);
        body.Children.Add(headingRow);
        body.Children.Add(Content as UIElement ?? new TextBlock { Text = Content?.ToString(), TextWrapping = TextWrapping.Wrap });
        var footer = new Border { Padding = new Thickness(24), BorderThickness = new Thickness(0, 1, 0, 0), Child = buttons };
        footer.SetResourceReference(Border.BackgroundProperty, "SolidBackgroundFillColorBaseBrush");
        footer.SetResourceReference(Border.BorderBrushProperty, "CardStrokeColorDefaultBrush");
        var layout = new DockPanel();
        DockPanel.SetDock(footer, Dock.Bottom);
        layout.Children.Add(footer);
        layout.Children.Add(new ScrollViewer { Content = body, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled });
        window.Content = layout;
        window.ShowDialog();
        return result;
    }
}
