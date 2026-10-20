import Cocoa
import SwiftUI

private let menuContentWidth: CGFloat = 336
private let providerDashboardMinimumHeight: CGFloat = 120
private let providerQuotaRowHeight: CGFloat = 60
private let providerQuotaRowSpacing: CGFloat = 6
private let visibleProviderQuotaRows = 3
// Padding, the tab row, and the divider above the quota section.
private let providerDashboardChromeHeight: CGFloat = 72

// Activity-style rings: one concentric ring per quota window. More than four
// rings stop being legible at menu size, so extra windows stay legend-only.
// 76pt keeps a one-window provider within the menu height budget.
private let quotaRingDiameter: CGFloat = 76
// Rings share a fixed band so the hole keeps room for the percentage label;
// more rings means thinner strokes rather than a smaller center.
private let quotaRingMaximumLineWidth: CGFloat = 9
private let quotaRingBand: CGFloat = 18
private let quotaRingGap: CGFloat = 2
private let maximumQuotaRings = 4
// One hue per ring so concentric windows stay distinguishable. Red is left
// out because it marks windows that are close to their limit.
private let quotaRingColors: [Color] = [.blue, .green, .orange, .purple]
private let quotaLegendRowHeight: CGFloat = 30
private let quotaLegendRowSpacing: CGFloat = 4
private let quotaWarningRatio = 0.9

enum QuotaDisplayStyle: String, CaseIterable, Identifiable {
    case ring
    case bar

    static let defaultsKey = "quotaDisplayStyle"

    var id: String { rawValue }
    var symbolName: String { self == .ring ? "circle.circle" : "line.3.horizontal" }
    var title: String {
        AppLocalization.string(self == .ring ? "quota.style.ring" : "quota.style.bar")
    }

    static func stored(_ defaults: UserDefaults = .standard) -> QuotaDisplayStyle {
        defaults.string(forKey: defaultsKey).flatMap(QuotaDisplayStyle.init(rawValue:)) ?? .ring
    }
}

private func providerQuotaSectionHeight(_ quotaCount: Int, style: QuotaDisplayStyle) -> CGFloat {
    let rowCount = max(1, min(quotaCount, visibleProviderQuotaRows))
    switch style {
    case .bar:
        return CGFloat(rowCount) * providerQuotaRowHeight
            + CGFloat(rowCount - 1) * providerQuotaRowSpacing
    case .ring:
        let legendHeight = CGFloat(rowCount) * quotaLegendRowHeight
            + CGFloat(rowCount - 1) * quotaLegendRowSpacing
        return max(quotaRingDiameter, legendHeight)
    }
}

/// Used fraction of a quota window; nil when the CLI reports no usable limit.
func providerQuotaRatio(_ usage: ProviderQuotaUsage) -> Double? {
    guard usage.error == nil, let limit = usage.limit, limit > 0 else { return nil }
    if let used = usage.used { return max(used / limit, 0) }
    if let remaining = usage.remaining { return max((limit - remaining) / limit, 0) }
    return nil
}

struct ProviderDashboardProvider: Equatable {
    let id: String
    let displayName: String
    let isEnabled: Bool
    let websiteURL: String?

    init(id: String, displayName: String, isEnabled: Bool = true, websiteURL: String? = nil) {
        self.id = id
        self.displayName = displayName
        self.isEnabled = isEnabled
        self.websiteURL = websiteURL
    }
}

struct ProviderUsageGroup {
    let providerID: String
    let displayName: String
    let websiteURL: String?
    let quotas: [ProviderQuotaUsage]
}

private func providerLogoAssetName(_ providerID: String) -> String {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") { return "baidu" }
    if normalized.contains("deepseek") { return "deepseek" }
    if normalized.contains("opencode") { return "opencode" }
    if normalized.contains("openrouter") { return "openrouter" }
    if normalized.contains("aws") || normalized.contains("bedrock") { return "aws" }
    if normalized.contains("openai") || normalized.contains("chatgpt") { return "openai" }
    return "custom"
}

private func providerBrandColor(_ providerID: String) -> Color {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") { return Color(red: 0.16, green: 0.29, blue: 0.93) }
    if normalized.contains("deepseek") { return Color(red: 0.34, green: 0.53, blue: 1) }
    if normalized.contains("opencode") { return .primary }
    if normalized.contains("openrouter") { return Color(red: 0.42, green: 0.44, blue: 0.95) }
    if normalized.contains("aws") || normalized.contains("bedrock") {
        return Color(red: 1.00, green: 0.60, blue: 0.00)
    }
    if normalized.contains("openai") || normalized.contains("chatgpt") {
        return Color(red: 0.10, green: 0.68, blue: 0.56)
    }
    return .secondary
}

private func providerLogoImage(_ providerID: String, websiteURL: String?) -> NSImage? {
    if let cached = cachedProviderLogoImage(providerID: providerID, websiteURL: websiteURL) {
        return cached
    }
    let assetName = providerLogoAssetName(providerID)
    let directories = [
        Bundle.main.resourceURL?.appendingPathComponent("ProviderLogos", isDirectory: true),
        Bundle.main.bundleURL.appendingPathComponent("ProviderLogos", isDirectory: true),
    ]
    for directory in directories.compactMap({ $0 }) {
        let url = directory.appendingPathComponent("\(assetName).svg")
        if let image = NSImage(contentsOf: url) {
            image.isTemplate = true
            return image
        }
    }
    return nil
}

private func providerMonogram(_ providerID: String) -> String {
    let letters = providerID
        .split(separator: "-")
        .prefix(2)
        .compactMap(\.first)
        .map(String.init)
        .joined()
    return (letters.isEmpty ? String(providerID.prefix(2)) : letters).uppercased()
}

func providerQuotaText(_ usage: ProviderQuotaUsage) -> String {
    let currency = usage.currency.map { " \($0)" } ?? ""
    if let used = usage.used, let limit = usage.limit {
        return "\(formatQuotaAmount(used)) / \(formatQuotaAmount(limit))\(currency)"
    }
    if let used = usage.used { return "\(formatQuotaAmount(used))\(currency)" }
    if let remaining = usage.remaining {
        return AppLocalization.string("menuViews.balance", formatQuotaAmount(remaining), currency)
    }
    if usage.error?.contains("not configured") == true {
        return AppLocalization.string("menuViews.quotaEndpointNotConfigured")
    }
    return AppLocalization.string("menuViews.queryFailed")
}

func providerQuotaLabel(_ usage: ProviderQuotaUsage, multiple: Bool) -> String {
    switch usage.quotaID {
    case "five_hour": return "5h"
    case "weekly": return "1 周"
    case "monthly": return "月度"
    case "balance": return "余额"
    case "quota": return multiple ? (usage.label ?? "额度") : "额度"
    default:
        if let label = usage.label?.trimmingCharacters(in: .whitespacesAndNewlines),
           !label.isEmpty {
            return label
        }
        return usage.menuLabel
    }
}

@MainActor
final class ProviderUsageDashboardModel: ObservableObject {
    var onContentHeightChange: ((CGFloat) -> Void)?
    @Published var configuredProviders: [ProviderDashboardProvider] = []
    @Published var quotaUsages: [ProviderQuotaUsage] = []
    @Published var quotaStatusTitle = "额度：检查中..."
    @Published var quotaStatusDetail: String?
    @Published var selectedProviderID: String?
    @Published private(set) var quotaStyle = QuotaDisplayStyle.stored()

    var groups: [ProviderUsageGroup] {
        configuredProviders.filter(\.isEnabled).map { provider in
            ProviderUsageGroup(
                providerID: provider.id,
                displayName: provider.displayName,
                websiteURL: provider.websiteURL,
                quotas: quotaUsages.filter { $0.providerID == provider.id }
            )
        }
    }

    var selectedGroup: ProviderUsageGroup? {
        groups.first { $0.providerID == selectedProviderID }
    }

    var contentHeight: CGFloat {
        guard let group = selectedGroup else { return providerDashboardMinimumHeight }
        return max(
            providerDashboardMinimumHeight,
            providerDashboardChromeHeight
                + providerQuotaSectionHeight(group.quotas.count, style: quotaStyle)
        )
    }

    func normalizeSelection() {
        if !groups.contains(where: { $0.providerID == selectedProviderID }) {
            selectedProviderID = groups.first?.providerID
        }
        onContentHeightChange?(contentHeight)
    }

    func selectProvider(_ providerID: String) {
        selectedProviderID = providerID
        onContentHeightChange?(contentHeight)
    }

    func selectQuotaStyle(_ style: QuotaDisplayStyle) {
        guard style != quotaStyle else { return }
        quotaStyle = style
        UserDefaults.standard.set(style.rawValue, forKey: QuotaDisplayStyle.defaultsKey)
        onContentHeightChange?(contentHeight)
    }
}

private struct ProviderUsageDashboardContent: View {
    @ObservedObject var model: ProviderUsageDashboardModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            providerTabs
            Divider()

            if let group = model.selectedGroup {
                quotaContent(group)
            } else {
                VStack(spacing: 8) {
                    Image(systemName: "chart.pie")
                        .font(.title2)
                        .foregroundStyle(.secondary)
                    Text(model.quotaStatusTitle)
                        .font(.headline)
                        .multilineTextAlignment(.center)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .help(model.quotaStatusDetail ?? "")
            }
        }
        .padding(10)
        .frame(width: menuContentWidth, height: model.contentHeight, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    // The tab strip already names the provider, so the quota style toggle
    // shares its row instead of a separate provider header.
    private var providerTabs: some View {
        HStack(spacing: 6) {
            ScrollView(.horizontal) {
                HStack(spacing: 6) {
                    ForEach(model.groups, id: \.providerID) { group in
                        let selected = group.providerID == model.selectedProviderID
                        Button {
                            model.selectProvider(group.providerID)
                        } label: {
                            ProviderLogoView(group: group)
                                .padding(4)
                                .background(
                                    providerBrandColor(group.providerID).opacity(selected ? 0.14 : 0),
                                    in: RoundedRectangle(cornerRadius: 8)
                                )
                                .overlay {
                                    RoundedRectangle(cornerRadius: 8)
                                        .stroke(providerBrandColor(group.providerID).opacity(selected ? 0.35 : 0))
                                }
                        }
                        .buttonStyle(.plain)
                        .help(group.displayName)
                        .accessibilityIdentifier("provider-tab-\(group.providerID)")
                    }
                }
            }
            .scrollIndicators(.hidden)
            if let group = model.selectedGroup, !group.quotas.isEmpty {
                Picker(AppLocalization.string("quota.style.help"), selection: Binding(
                    get: { model.quotaStyle },
                    set: model.selectQuotaStyle
                )) {
                    ForEach(QuotaDisplayStyle.allCases) { style in
                        Image(systemName: style.symbolName)
                            .accessibilityLabel(style.title)
                            .tag(style)
                    }
                }
                .labelsHidden()
                .pickerStyle(.segmented)
                .controlSize(.mini)
                .fixedSize()
                .help(AppLocalization.string("quota.style.help"))
                .accessibilityIdentifier("quota-style-picker")
            }
        }
        .frame(height: 34)
    }

    @ViewBuilder
    private func quotaContent(_ group: ProviderUsageGroup) -> some View {
        if group.quotas.isEmpty {
            Text(model.quotaStatusTitle)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .help(model.quotaStatusDetail ?? "")
        } else if model.quotaStyle == .ring {
            ProviderQuotaRings(usages: group.quotas, multiple: group.quotas.count > 1)
            .frame(height: providerQuotaSectionHeight(group.quotas.count, style: .ring))
        } else {
            let quotaRows = VStack(spacing: providerQuotaRowSpacing) {
                ForEach(Array(group.quotas.enumerated()), id: \.offset) { _, usage in
                    ProviderQuotaRow(usage: usage, multiple: group.quotas.count > 1)
                        .frame(height: providerQuotaRowHeight)
                }
            }
            Group {
                if group.quotas.count > visibleProviderQuotaRows {
                    ScrollView(.vertical) {
                        quotaRows
                    }
                    .scrollIndicators(.visible)
                } else {
                    quotaRows
                }
            }
            .frame(height: providerQuotaSectionHeight(group.quotas.count, style: .bar))
        }
    }

}

private struct ProviderLogoView: View {
    let group: ProviderUsageGroup

    var body: some View {
        Group {
            if let image = providerLogoImage(group.providerID, websiteURL: group.websiteURL) {
                Image(nsImage: image).resizable().scaledToFit()
            } else {
                Text(providerMonogram(group.providerID))
                    .font(.caption2.weight(.bold))
            }
        }
        .foregroundStyle(providerBrandColor(group.providerID))
        .frame(width: 22, height: 22)
    }
}

private struct ProviderQuotaRow: View {
    let usage: ProviderQuotaUsage
    let multiple: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .top, spacing: 8) {
                Text(providerQuotaLabel(usage, multiple: multiple))
                    .fontWeight(.semibold)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(providerQuotaText(usage)).fontDesign(.monospaced)
                    .fixedSize(horizontal: true, vertical: false)
            }
            .font(.caption2)
            if let ratio = providerQuotaRatio(usage) {
                ProgressView(value: min(ratio, 1))
                    .tint(ratio >= quotaWarningRatio ? .red : nil)
            }
            if let resetAt = usage.resetAt, let date = ISO8601DateFormatter().date(from: resetAt) {
                Text(AppLocalization.string("quota.resetsAt", date.formatted(date: .abbreviated, time: .shortened)))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .help(usage.error ?? "")
    }
}

private struct ProviderQuotaRings: View {
    let usages: [ProviderQuotaUsage]
    let multiple: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var revealed = false

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            ZStack {
                ForEach(Array(usages.prefix(maximumQuotaRings).enumerated()), id: \.offset) { index, usage in
                    ring(index: index, usage: usage)
                }
                centerLabel
            }
            .frame(width: quotaRingDiameter, height: quotaRingDiameter)
            .accessibilityHidden(true)

            legend
        }
        .onAppear {
            guard !revealed else { return }
            withAnimation(reduceMotion ? nil : .spring(response: 0.9, dampingFraction: 0.82)) {
                revealed = true
            }
        }
    }

    private func ringColor(_ index: Int) -> Color {
        index < quotaRingColors.count ? quotaRingColors[index] : Color.secondary.opacity(0.4)
    }

    private func ring(index: Int, usage: ProviderQuotaUsage) -> some View {
        let count = CGFloat(min(usages.count, maximumQuotaRings))
        let lineWidth = min(quotaRingMaximumLineWidth, (quotaRingBand - quotaRingGap * (count - 1)) / count)
        let inset = CGFloat(index) * (lineWidth + quotaRingGap) + lineWidth / 2
        let fill = min(providerQuotaRatio(usage) ?? 0, 1)
        let color = ringColor(index)
        return ZStack {
            Circle()
                .stroke(color.opacity(0.18), lineWidth: lineWidth)
            Circle()
                .trim(from: 0, to: revealed ? fill : 0)
                .stroke(color, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .padding(inset)
    }

    @ViewBuilder
    private var centerLabel: some View {
        // The center shows the tightest window, the one that runs out first.
        if let ratio = usages.compactMap(providerQuotaRatio).max() {
            Text(quotaPercentText(ratio))
                .font(.system(size: 15, weight: .bold).monospacedDigit())
                .minimumScaleFactor(0.7)
                .foregroundStyle(ratio >= quotaWarningRatio ? Color.red : Color.primary)
        }
    }

    private var legend: some View {
        let rows = VStack(alignment: .leading, spacing: quotaLegendRowSpacing) {
            ForEach(Array(usages.enumerated()), id: \.offset) { index, usage in
                legendRow(index: index, usage: usage)
                    .frame(height: quotaLegendRowHeight)
            }
        }
        return Group {
            if usages.count > visibleProviderQuotaRows {
                ScrollView(.vertical) { rows }.scrollIndicators(.visible)
            } else {
                rows
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func legendRow(index: Int, usage: ProviderQuotaUsage) -> some View {
        let ratio = providerQuotaRatio(usage)
        let warning = (ratio ?? 0) >= quotaWarningRatio
        let resetText = usage.resetAt
            .flatMap { ISO8601DateFormatter().date(from: $0) }
            .map { AppLocalization.string("quota.resetsAt", $0.formatted(date: .abbreviated, time: .shortened)) }
        let detail = [providerQuotaText(usage), resetText].compactMap { $0 }.joined(separator: " · ")
        return VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 5) {
                Circle()
                    .fill(ringColor(index))
                    .frame(width: 7, height: 7)
                Text(providerQuotaLabel(usage, multiple: multiple))
                    .fontWeight(.semibold)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 4)
                Text(ratio.map(quotaPercentText) ?? "—")
                    .fontWeight(.semibold)
                    .monospacedDigit()
                    .foregroundStyle(warning ? Color.red : Color.primary)
            }
            Text(detail)
                .fontDesign(.monospaced)
                .foregroundStyle(usage.error == nil ? Color.secondary : Color.red)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .font(.caption2)
        .help(usage.error ?? detail)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(providerQuotaLabel(usage, multiple: multiple)) \(detail)")
        .accessibilityValue(ratio.map(quotaPercentText) ?? "")
    }
}

private func quotaPercentText(_ ratio: Double) -> String {
    "\(Int((ratio * 100).rounded()))%"
}

final class ProviderUsageDashboardView: FlippedMenuView {
    let model = ProviderUsageDashboardModel()
    private let hostingView: NSHostingView<ProviderUsageDashboardContent>
    /// Tracks whether a coalesced row-height sync is already queued so repeated
    /// data refreshes collapse into a single deferred resize.
    private var heightSyncScheduled = false

    init() {
        hostingView = NSHostingView(rootView: ProviderUsageDashboardContent(model: model))
        super.init(frame: NSRect(x: 0, y: 0, width: menuContentWidth, height: providerDashboardMinimumHeight))
        installHostingView()
    }

    required init?(coder: NSCoder) {
        hostingView = NSHostingView(rootView: ProviderUsageDashboardContent(model: model))
        super.init(coder: coder)
        frame.size = NSSize(width: menuContentWidth, height: providerDashboardMinimumHeight)
        installHostingView()
    }

    func updateQuotaStatus(title: String, detail: String?) {
        model.quotaStatusTitle = title
        model.quotaStatusDetail = detail
        scheduleContentHeightUpdate()
    }

    func updateConfiguredProviders(_ providers: [ProviderDashboardProvider]) {
        model.configuredProviders = providers
        model.normalizeSelection()
        scheduleContentHeightUpdate()
    }

    func refreshProviderIcons() {
        hostingView.rootView = ProviderUsageDashboardContent(model: model)
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        model.quotaUsages = usages
        model.quotaStatusTitle = L10n.Provider.quotaEmpty
        model.quotaStatusDetail = nil
        model.normalizeSelection()
        scheduleContentHeightUpdate()
    }

    private func installHostingView() {
        hostingView.frame = bounds
        hostingView.autoresizingMask = [.width, .height]
        addSubview(hostingView)
        model.onContentHeightChange = { [weak self] _ in
            self?.scheduleContentHeightUpdate()
        }
    }

    /// Defers custom-row height changes out of the synchronous SwiftUI callback
    /// that requested them, while leaving the width managed by `NSMenu` intact.
    private func scheduleContentHeightUpdate() {
        let targetHeight = model.contentHeight
        guard frame.height != targetHeight else { return }
        guard !heightSyncScheduled else { return }
        heightSyncScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.heightSyncScheduled = false
            self.applyContentHeight()
        }
    }

    private func applyContentHeight() {
        let targetHeight = model.contentHeight
        guard frame.height != targetHeight else { return }
        frame.size.height = targetHeight
        hostingView.frame = bounds
    }
}


func formatQuotaAmount(_ value: Double) -> String {
    let formatter = NumberFormatter()
    formatter.minimumFractionDigits = value.rounded() == value ? 0 : 2
    formatter.maximumFractionDigits = 2
    return formatter.string(from: NSNumber(value: value)) ?? String(format: "%.2f", value)
}
