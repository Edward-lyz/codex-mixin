using Microsoft.UI.Xaml.Controls;
using Windows.Foundation;

namespace CodexMixin;

// Native controls keep their own measurement; actions wrap on narrow windows.
internal sealed class ActionPanel : Panel
{
    private const double Gap = 8;

    protected override Size MeasureOverride(Size availableSize)
    {
        double left = 0, top = 0, rowHeight = 0, width = 0;
        foreach (var child in Children)
        {
            child.Measure(availableSize);
            var size = child.DesiredSize;
            if (left > 0 && left + size.Width > availableSize.Width)
            {
                top += rowHeight + Gap; left = 0; rowHeight = 0;
            }
            width = Math.Max(width, left + size.Width);
            left += size.Width + Gap;
            rowHeight = Math.Max(rowHeight, size.Height);
        }
        return new Size(width, top + rowHeight);
    }

    protected override Size ArrangeOverride(Size finalSize)
    {
        double left = 0, top = 0, rowHeight = 0;
        foreach (var child in Children)
        {
            var size = child.DesiredSize;
            var width = Math.Min(size.Width, finalSize.Width);
            if (left > 0 && left + width > finalSize.Width)
            {
                top += rowHeight + Gap; left = 0; rowHeight = 0;
            }
            child.Arrange(new Rect(left, top, width, size.Height));
            left += width + Gap;
            rowHeight = Math.Max(rowHeight, size.Height);
        }
        return finalSize;
    }
}
