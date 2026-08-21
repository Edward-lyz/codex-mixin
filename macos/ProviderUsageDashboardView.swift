import Cocoa
import SwiftUI

private let menuContentWidth: CGFloat = 336
private let providerDashboardMinimumHeight: CGFloat = 168

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
    let models: [ProviderTokenUsage]
}

enum TokenUsageRange: String, CaseIterable, Identifiable {
    case day
    case week
    case month
    case all

    var id: String { rawValue }
    var title: String {
        switch self {
        case .day: return "1天"
        case .week: return "7天"
        case .month: return "1月"
        case .all: return "全部"
        }
    }
    var days: Int? {
        switch self {
        case .day: return 1
        case .week: return 7
        case .month: return 30
        case .all: return nil
        }
    }
    var commandArguments: [String] {
        guard let days else { return ["usage", "--json"] }
        return ["usage", "--json", "--days", "\(days)"]
    }
}

private func providerLogoAssetName(_ providerID: String) -> String {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") { return "baidu" }
    if normalized.contains("deepseek") { return "deepseek" }
    if normalized.contains("opencode") { return "opencode" }
    if normalized.contains("openrouter") { return "openrouter" }
    if normalized.contains("openai") || normalized.contains("chatgpt") { return "openai" }
    return "custom"
}

private func providerBrandColor(_ providerID: String) -> Color {
    let normalized = providerID.lowercased()
    if normalized.contains("baidu") { return Color(red: 0.16, green: 0.29, blue: 0.93) }
    if normalized.contains("deepseek") { return Color(red: 0.34, green: 0.53, blue: 1) }
    if normalized.contains("opencode") { return .primary }
    if normalized.contains("openrouter") { return Color(red: 0.42, green: 0.44, blue: 0.95) }
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

func tokenUsageDetail(_ usage: ProviderTokenUsage) -> String {
    let cacheRatio = usage.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "未上报"
    return """
    输入 \(formatTokenCount(usage.inputTokens))
    缓存输入 \(formatTokenCount(usage.cacheReadTokens))
    输出 \(formatTokenCount(usage.outputTokens))
    缓存输出 \(formatTokenCount(usage.cacheCreationTokens))
    整体缓存比例 \(cacheRatio)
    """
}

@MainActor
final class ProviderUsageDashboardModel: ObservableObject {
    var onRangeChange: ((TokenUsageRange) -> Void)?
    var onContentHeightChange: ((CGFloat) -> Void)?
    @Published var configuredProviders: [ProviderDashboardProvider] = []
    @Published var quotaUsages: [ProviderQuotaUsage] = []
    @Published var tokenUsages: [ProviderTokenUsage] = []
    @Published var quotaStatusTitle = "额度：检查中..."
    @Published var quotaStatusDetail: String?
    @Published var tokenStatusTitle = "Token 使用：检查中..."
    @Published var tokenStatusDetail: String?
    @Published var selectedProviderID: String?
    @Published var selectedModelID: String?
    @Published var selectedRange = TokenUsageRange.all

    var groups: [ProviderUsageGroup] {
        configuredProviders.filter(\.isEnabled).map { provider in
            ProviderUsageGroup(
                providerID: provider.id,
                displayName: provider.displayName,
                websiteURL: provider.websiteURL,
                quotas: quotaUsages.filter { $0.providerID == provider.id },
                models: tokenUsages
                    .filter { $0.providerID == provider.id }
                    .sorted {
                        $0.totalTokens == $1.totalTokens
                            ? $0.modelID < $1.modelID
                            : $0.totalTokens > $1.totalTokens
                    }
            )
        }
    }

    var selectedGroup: ProviderUsageGroup? {
        groups.first { $0.providerID == selectedProviderID }
    }

    var selectedModel: ProviderTokenUsage? {
        selectedGroup?.models.first { $0.modelID == selectedModelID }
    }

    var contentHeight: CGFloat {
        guard let group = selectedGroup else { return providerDashboardMinimumHeight }
        let quotaRows = max(1, group.quotas.count)
        let tokenHeight: CGFloat = group.models.isEmpty ? 20 : 144
        let detailHeight: CGFloat = selectedModel == nil ? 0 : 104
        return max(providerDashboardMinimumHeight, 118 + CGFloat(quotaRows * 28) + tokenHeight + detailHeight)
    }

    func normalizeSelection() {
        if !groups.contains(where: { $0.providerID == selectedProviderID }) {
            selectedProviderID = groups.first?.providerID
            selectedModelID = nil
        }
        if let selectedModelID,
           selectedGroup?.models.contains(where: { $0.modelID == selectedModelID }) != true {
            self.selectedModelID = nil
        }
        onContentHeightChange?(contentHeight)
    }

    func selectProvider(_ providerID: String) {
        selectedProviderID = providerID
        selectedModelID = nil
        onContentHeightChange?(contentHeight)
    }

    func selectModel(_ modelID: String) {
        selectedModelID = selectedModelID == modelID ? nil : modelID
        onContentHeightChange?(contentHeight)
    }

    func selectRange(_ range: TokenUsageRange) {
        guard range != selectedRange else { return }
        selectedRange = range
        selectedModelID = nil
        onRangeChange?(range)
        onContentHeightChange?(contentHeight)
    }
}

private struct ProviderUsageDashboardContent: View {
    @ObservedObject var model: ProviderUsageDashboardModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Provider 使用")
                .font(.caption.weight(.semibold))

            providerTabs
            Divider()

            if let group = model.selectedGroup {
                providerSummary(group)
                quotaContent(group)
                tokenContent(group)
            } else {
                ContentUnavailableView(
                    model.tokenStatusTitle,
                    systemImage: "chart.bar.xaxis"
                )
                .help(model.tokenStatusDetail ?? model.quotaStatusDetail ?? "")
            }
        }
        .padding(10)
        .frame(width: menuContentWidth, height: model.contentHeight, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var providerTabs: some View {
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
        .frame(height: 34)
    }

    private func providerSummary(_ group: ProviderUsageGroup) -> some View {
        HStack(spacing: 8) {
            ProviderLogoView(group: group)
            Text(group.displayName)
                .font(.callout.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    @ViewBuilder
    private func quotaContent(_ group: ProviderUsageGroup) -> some View {
        if group.quotas.isEmpty {
            Text(model.quotaStatusTitle)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .help(model.quotaStatusDetail ?? "")
        } else {
            VStack(spacing: 6) {
                ForEach(Array(group.quotas.enumerated()), id: \.offset) { _, usage in
                    ProviderQuotaRow(usage: usage, multiple: group.quotas.count > 1)
                }
            }
            .frame(height: CGFloat(group.quotas.count * 28))
        }
    }

    @ViewBuilder
    private func tokenContent(_ group: ProviderUsageGroup) -> some View {
        VStack(spacing: 7) {
            Picker("统计口径", selection: Binding(
                get: { model.selectedRange },
                set: model.selectRange
            )) {
                ForEach(TokenUsageRange.allCases) { range in
                    Text(range.title).tag(range)
                }
            }
            .labelsHidden()
            .pickerStyle(.segmented)
            .controlSize(.mini)

            if group.models.isEmpty {
            Text(model.tokenStatusTitle)
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .frame(maxWidth: .infinity, alignment: .center)
                .help(model.tokenStatusDetail ?? "")
            } else {
                let maximumTokens = group.models.map(\.totalTokens).max() ?? 0
                ScrollView(.horizontal) {
                    HStack(alignment: .bottom, spacing: 3) {
                        ForEach(group.models, id: \.modelID) { usage in
                            Button {
                                model.selectModel(usage.modelID)
                            } label: {
                                TokenModelColumn(
                                    usage: usage,
                                    maximumTokens: maximumTokens,
                                    selected: usage.modelID == model.selectedModelID
                                )
                            }
                            .buttonStyle(.plain)
                            .help(tokenUsageDetail(usage))
                            .accessibilityIdentifier("token-model-\(usage.modelID)")
                        }
                    }
                    .padding(.horizontal, 4)
                }
                .scrollIndicators(.hidden)
                .frame(height: 104)

                if let selectedModel = model.selectedModel {
                    TokenModelDetail(usage: selectedModel)
                }
            }
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
        VStack(spacing: 3) {
            HStack {
                Text(providerQuotaLabel(usage, multiple: multiple)).fontWeight(.semibold)
                Spacer()
                Text(providerQuotaText(usage)).fontDesign(.monospaced)
            }
            .font(.caption2)
            if let used = usage.used, let limit = usage.limit, limit > 0 {
                ProgressView(value: min(max(used / limit, 0), 1))
            }
        }
        .help(usage.error ?? "")
    }
}

private struct TokenModelColumn: View {
    let usage: ProviderTokenUsage
    let maximumTokens: UInt64
    let selected: Bool

    var body: some View {
        VStack(spacing: 4) {
            Text(formatTokenCount(usage.totalTokens))
                .font(.system(size: 9, weight: selected ? .semibold : .medium).monospacedDigit())
                .foregroundStyle(selected ? Color.primary : Color.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(width: 47)
            TokenVerticalBar(usage: usage, maximumTokens: maximumTokens, selected: selected)
            Text(usage.modelID)
                .font(.system(size: 9, weight: selected ? .semibold : .regular))
                .foregroundStyle(selected ? Color.accentColor : Color.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: 47)
        }
        .contentShape(Rectangle())
    }
}

private struct TokenVerticalBar: View {
    let usage: ProviderTokenUsage
    let maximumTokens: UInt64
    let selected: Bool

    var body: some View {
        VStack {
            Spacer(minLength: 0)
            Capsule()
                .fill(Color.accentColor.opacity(selected ? 1 : 0.78))
                .frame(width: selected ? 9 : 7, height: barHeight)
        }
        .frame(width: 11, height: 66)
    }

    private var barHeight: CGFloat {
        maximumTokens == 0
            ? 0
            : max(3, 66 * CGFloat(Double(usage.totalTokens) / Double(maximumTokens)))
    }
}

private struct TokenModelDetail: View {
    let usage: ProviderTokenUsage

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(usage.modelID)
                .font(.caption2.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)
            Grid(horizontalSpacing: 10, verticalSpacing: 6) {
                GridRow {
                    metric("请求", "\(usage.requestCount)")
                    metric("输入", formatTokenCount(usage.inputTokens))
                    metric("缓存输入", formatTokenCount(usage.cacheReadTokens))
                }
                GridRow {
                    metric("输出", formatTokenCount(usage.outputTokens))
                    metric("缓存输出", formatTokenCount(usage.cacheCreationTokens))
                    metric("缓存比例", usage.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "未上报")
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(.separator.opacity(0.35))
        }
        .help(tokenUsageDetail(usage))
    }

    private func metric(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.system(size: 8)).foregroundStyle(.tertiary)
            Text(value).font(.system(size: 10, weight: .semibold).monospacedDigit())
                .lineLimit(1)
                .frame(minWidth: 76, alignment: .leading)
        }
    }
}

final class ProviderUsageDashboardView: FlippedMenuView {
    let model = ProviderUsageDashboardModel()
    private let hostingView: NSHostingView<ProviderUsageDashboardContent>

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
        updateSize()
    }

    func updateConfiguredProviders(_ providers: [ProviderDashboardProvider]) {
        model.configuredProviders = providers
        model.normalizeSelection()
        updateSize()
    }

    func refreshProviderIcons() {
        hostingView.rootView = ProviderUsageDashboardContent(model: model)
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        model.quotaUsages = usages
        model.quotaStatusTitle = L10n.Provider.quotaEmpty
        model.quotaStatusDetail = nil
        model.normalizeSelection()
        updateSize()
    }

    func updateTokenStatus(title: String, detail: String?) {
        model.tokenStatusTitle = title
        model.tokenStatusDetail = detail
        updateSize()
    }

    func updateTokenUsages(_ usages: [ProviderTokenUsage]) {
        model.tokenUsages = usages
        model.tokenStatusTitle = "Token 使用：暂无数据"
        model.tokenStatusDetail = nil
        model.normalizeSelection()
        updateSize()
    }

    var onRangeChange: ((TokenUsageRange) -> Void)? {
        get { model.onRangeChange }
        set { model.onRangeChange = newValue }
    }

    private func installHostingView() {
        hostingView.frame = bounds
        hostingView.autoresizingMask = [.width, .height]
        addSubview(hostingView)
        model.onContentHeightChange = { [weak self] _ in
            self?.updateSize()
        }
    }

    private func updateSize() {
        frame.size = NSSize(width: menuContentWidth, height: model.contentHeight)
        hostingView.frame = bounds
    }
}

func formatTokenCount(_ count: UInt64) -> String {
    if count >= 1_000_000 { return String(format: "%.1fM", Double(count) / 1_000_000) }
    if count >= 1_000 { return String(format: "%.1fk", Double(count) / 1_000) }
    return "\(count)"
}

func formatQuotaAmount(_ value: Double) -> String {
    let formatter = NumberFormatter()
    formatter.minimumFractionDigits = value.rounded() == value ? 0 : 2
    formatter.maximumFractionDigits = 2
    return formatter.string(from: NSNumber(value: value)) ?? String(format: "%.2f", value)
}
