import Cocoa
import SwiftUI

private let menuContentWidth: CGFloat = 336
private let providerDashboardMinimumHeight: CGFloat = 120
private let providerQuotaRowHeight: CGFloat = 60
private let providerQuotaRowSpacing: CGFloat = 6
private let visibleProviderQuotaRows = 3
// Padding, the tab row, and the divider above the quota section.
private let providerDashboardChromeHeight: CGFloat = 72

private let quotaWarningRatio = 0.8
private let quotaCriticalRatio = 0.95

private func providerQuotaSectionHeight(_ quotaCount: Int) -> CGFloat {
    let rowCount = max(1, min(quotaCount, visibleProviderQuotaRows))
    return CGFloat(rowCount) * providerQuotaRowHeight
        + CGFloat(rowCount - 1) * providerQuotaRowSpacing
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
    if normalized == "official" || normalized.contains("openai") || normalized.contains("chatgpt") { return "openai" }
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

private let dashboardBundledLogoCache = NSCache<NSString, NSImage>()

func providerLogoImage(_ providerID: String, websiteURL: String?) -> NSImage? {
    let assetName = providerLogoAssetName(providerID)
    if assetName == "custom",
       let cached = cachedProviderLogoImage(providerID: providerID, websiteURL: websiteURL) {
        return cached
    }
    if let image = dashboardBundledLogoCache.object(forKey: assetName as NSString) { return image }
    dashboardBundledLogoCache.countLimit = 16
    let directories = [
        Bundle.main.resourceURL?.appendingPathComponent("ProviderLogos", isDirectory: true),
        Bundle.main.bundleURL.appendingPathComponent("ProviderLogos", isDirectory: true),
    ]
    for directory in directories.compactMap({ $0 }) {
        let url = directory.appendingPathComponent("\(assetName).svg")
        if let image = NSImage(contentsOf: url) {
            image.isTemplate = true
            dashboardBundledLogoCache.setObject(image, forKey: assetName as NSString)
            return image
        }
    }
    return cachedProviderLogoImage(providerID: providerID, websiteURL: websiteURL)
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
                + providerQuotaSectionHeight(group.quotas.count)
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
    }

    // The tab strip already names the provider.
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
            .frame(height: providerQuotaSectionHeight(group.quotas.count))
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
                Text(providerQuotaRatio(usage).map { AppLocalization.string("quota.usedPercent", quotaPercentText($0)) } ?? providerQuotaText(usage))
                    .monospacedDigit()
                    .fixedSize(horizontal: true, vertical: false)
            }
            .font(.caption2)
            if let ratio = providerQuotaRatio(usage) {
                ProgressView(value: min(ratio, 1))
                    .tint(ratio >= quotaCriticalRatio ? .red : ratio >= quotaWarningRatio ? .yellow : nil)
            }
            if let detail = quotaDetailText(usage) {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .help(usage.error ?? "")
    }
}

/// Amount and reset time under a quota. A 100-point window repeats its
/// percentage, so only real amounts are spelled out.
private func quotaDetailText(_ usage: ProviderQuotaUsage) -> String? {
    if usage.error != nil { return providerQuotaText(usage) }
    let amount = (usage.limit == 100 && usage.currency == nil) ? nil : providerQuotaText(usage)
    let reset = usage.resetAt
        .flatMap { ISO8601DateFormatter().date(from: $0) }
        .map { date in
            let formatter = RelativeDateTimeFormatter()
            formatter.unitsStyle = .short
            return AppLocalization.string("quota.resetsIn", formatter.localizedString(for: date, relativeTo: Date()))
        }
    let parts = [amount, reset].compactMap { $0 }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
}

private func quotaPercentText(_ ratio: Double) -> String {
    "\(Int((ratio * 100).rounded()))%"
}

final class ProviderUsageDashboardView: FlippedMenuView {
    let model = ProviderUsageDashboardModel()
    private var hostingView: NSHostingView<ProviderUsageDashboardContent>?
    var isMenuVisible = false {
        didSet {
            guard isMenuVisible != oldValue else { return }
            if isMenuVisible {
                installHostingView()
            } else {
                hostingView?.removeFromSuperview()
                hostingView = nil
            }
        }
    }
    /// Tracks whether a coalesced row-height sync is already queued so repeated
    /// data refreshes collapse into a single deferred resize.
    private var heightSyncScheduled = false

    init() {
        super.init(frame: NSRect(x: 0, y: 0, width: menuContentWidth, height: providerDashboardMinimumHeight))
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        frame.size = NSSize(width: menuContentWidth, height: providerDashboardMinimumHeight)
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
        hostingView?.rootView = ProviderUsageDashboardContent(model: model)
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        model.quotaUsages = usages
        model.quotaStatusTitle = AppLocalization.string("provider.quotaEmpty")
        model.quotaStatusDetail = nil
        model.normalizeSelection()
        scheduleContentHeightUpdate()
    }

    private func installHostingView() {
        let content = NSHostingView(rootView: ProviderUsageDashboardContent(model: model))
        content.frame = bounds
        content.autoresizingMask = [.width, .height]
        addSubview(content)
        hostingView = content
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
        hostingView?.frame = bounds
    }
}


func formatQuotaAmount(_ value: Double) -> String {
    let formatter = NumberFormatter()
    formatter.minimumFractionDigits = value.rounded() == value ? 0 : 2
    formatter.maximumFractionDigits = 2
    return formatter.string(from: NSNumber(value: value)) ?? String(format: "%.2f", value)
}
