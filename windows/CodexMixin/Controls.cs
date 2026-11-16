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
