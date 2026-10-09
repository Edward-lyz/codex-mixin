import Cocoa
import SwiftUI

private let menuContentWidth: CGFloat = 336
private let providerDashboardMinimumHeight: CGFloat = 168
private let providerQuotaRowHeight: CGFloat = 28
private let providerQuotaRowSpacing: CGFloat = 6
private let visibleProviderQuotaRows = 3

private func providerQuotaSectionHeight(_ quotaCount: Int) -> CGFloat {
    let rowCount = max(1, min(quotaCount, visibleProviderQuotaRows))
    return CGFloat(rowCount) * providerQuotaRowHeight
        + CGFloat(rowCount - 1) * providerQuotaRowSpacing
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

struct RequestUsageRow: Decodable, Identifiable {
    let id: Int64
    let recordedAtMs: UInt64
    let clientID: String
    let providerID: String
    let modelID: String
    let inputTokens: UInt64
    let cacheReadTokens: UInt64
    let outputTokens: UInt64
    let ttftMicros: UInt64?
    let generationMicros: UInt64?
    let prefixState: String?

    enum CodingKeys: String, CodingKey {
        case id
        case recordedAtMs = "recorded_at_ms"
        case clientID = "client_id"
        case providerID = "provider_id"
        case modelID = "model_id"
        case inputTokens = "input_tokens"
        case cacheReadTokens = "cache_read_tokens"
        case outputTokens = "output_tokens"
        case ttftMicros = "ttft_micros"
        case generationMicros = "generation_micros"
        case prefixState = "prefix_state"
    }

    var ttftMs: Double? { ttftMicros.map { Double($0) / 1_000.0 } }
    var outputTPS: Double? {
        guard let micros = generationMicros, micros > 0 else { return nil }
        return Double(outputTokens) * 1_000_000.0 / Double(micros)
    }
}

func parseRequestUsage(_ rawJSON: String) throws -> [RequestUsageRow] {
    do {
        return try JSONDecoder().decode([RequestUsageRow].self, from: Data(rawJSON.utf8))
    } catch {
        throw GatewayError.command("请求用量 JSON 无法解析：\(error)")
    }
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

func tokenUsageDetail(_ usage: ProviderTokenUsage) -> String {
    let cacheRatio = usage.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "未上报"
    let ttft = usage.averageTTFTMs.map { String(format: "%.0f ms", $0) } ?? "未上报"
    let tps = usage.outputTPS.map { String(format: "%.1f tok/s", $0) } ?? "未上报"
    return """
    输入 \(formatTokenCount(usage.inputTokens))
    缓存输入 \(formatTokenCount(usage.cacheReadTokens))
    输出 \(formatTokenCount(usage.outputTokens))
    整体缓存比例 \(cacheRatio)
    首字响应 \(ttft)
    每秒吞吐 \(tps)
    """
}

@MainActor
final class ProviderUsageDashboardModel: ObservableObject {
    var onRangeChange: ((TokenUsageRange) -> Void)?
    var onContentHeightChange: ((CGFloat) -> Void)?
    var onRequestRefresh: (() -> Void)?
    @Published var configuredProviders: [ProviderDashboardProvider] = []
    @Published var quotaUsages: [ProviderQuotaUsage] = []
    @Published var tokenUsages: [ProviderTokenUsage] = []
    @Published var requestRows: [RequestUsageRow] = []
    @Published var requestStatus = "请求明细：检查中..."
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
        // Menu embed only renders provider icons + quota rows; model usage lives
        // in the detached window, so height tracks the quota section alone.
        return max(
            providerDashboardMinimumHeight,
            101 + providerQuotaSectionHeight(group.quotas.count)
        )
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
    let compact: Bool

    var body: some View {
        if compact {
            menuBody
        } else {
            windowBody
        }
    }

    private var menuBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            providerTabs
            Divider()
            if let group = model.selectedGroup {
                providerSummary(group)
                quotaContent(group)
            } else {
                emptyOverview
            }
        }
        .padding(10)
        .frame(width: menuContentWidth, height: model.contentHeight, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    /// Magpie-style window: provider sidebar on the left, one scrolling page
    /// on the right with stats, the model breakdown, then request details.
    private var windowBody: some View {
        HStack(spacing: 0) {
            UsageSidebar(model: model)
                .frame(width: 196)
            Divider()
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 32) {
                    pageHeader
                    if let group = model.selectedGroup {
                        UsageStatStrip(group: group)
                        modelRanking(group)
                        requestSection(group)
                    } else {
                        emptyOverview
                    }
                }
                .padding(.horizontal, 28)
                .padding(.vertical, 24)
                .frame(maxWidth: .infinity, alignment: .topLeading)
            }
        }
        .frame(minWidth: 760, maxWidth: .infinity, minHeight: 480, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var pageHeader: some View {
        HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 4) {
                Text(model.selectedGroup?.displayName ?? "用量")
                    .font(.system(size: 22, weight: .semibold))
                Text("Token、缓存与速度")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            rangePicker
        }
    }

    private var emptyOverview: some View {
        VStack(spacing: 8) {
            Image(systemName: "chart.bar.xaxis")
                .font(.title2)
                .foregroundStyle(.secondary)
            Text(model.tokenStatusTitle)
                .font(.headline)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .help(model.tokenStatusDetail ?? model.quotaStatusDetail ?? "")
    }

    private var rangePicker: some View {
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
        .frame(width: 220)
    }

    private func requestSection(_ group: ProviderUsageGroup) -> some View {
        let rows = model.requestRows.filter { $0.providerID == group.providerID }
        return VStack(alignment: .leading, spacing: 14) {
            Text("请求明细")
                .font(.system(size: 15, weight: .semibold))
            if rows.isEmpty {
                Text(model.requestRows.isEmpty ? model.requestStatus : "最近没有该供应商的请求")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 24)
            } else {
                RequestUsageTable(rows: rows)
            }
        }
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
        } else if compact {
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
        } else {
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 220), spacing: 12, alignment: .top)],
                alignment: .leading,
                spacing: 12
            ) {
                ForEach(Array(group.quotas.enumerated()), id: \.offset) { _, usage in
                    QuotaCard(usage: usage, multiple: group.quotas.count > 1)
                }
            }
        }
    }

    @ViewBuilder
    private func modelRanking(_ group: ProviderUsageGroup) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("按模型")
                .font(.system(size: 15, weight: .semibold))
            if group.models.isEmpty {
                Text(model.tokenStatusTitle)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 24)
                    .help(model.tokenStatusDetail ?? "")
            } else {
                ModelUsageChart(
                    models: group.models,
                    selectedModelID: model.selectedModelID,
                    onSelect: model.selectModel
                )
                if let selectedModel = model.selectedModel {
                    TokenModelDetail(usage: selectedModel)
                }
                ModelUsageTable(models: group.models)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Left navigation column listing providers.
private struct UsageSidebar: View {
    @ObservedObject var model: ProviderUsageDashboardModel

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            sectionTitle("供应商")
            ScrollView(.vertical) {
                VStack(spacing: 2) {
                    ForEach(model.groups, id: \.providerID) { group in
                        sidebarRow(selected: group.providerID == model.selectedProviderID) {
                            ProviderLogoView(group: group).frame(width: 18, height: 18)
                            Text(group.displayName).lineLimit(1).truncationMode(.tail)
                        } action: {
                            model.selectProvider(group.providerID)
                        }
                        .accessibilityIdentifier("provider-tab-\(group.providerID)")
                    }
                }
            }
            .scrollIndicators(.hidden)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 16)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private func sectionTitle(_ title: String) -> some View {
        Text(title)
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .padding(.bottom, 4)
    }

    private func sidebarRow<Label: View>(
        selected: Bool,
        @ViewBuilder label: () -> Label,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 8) { label() }
                .font(.system(size: 13, weight: selected ? .semibold : .regular))
                .padding(.horizontal, 8)
                .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                .background(
                    Color.primary.opacity(selected ? 0.08 : 0),
                    in: RoundedRectangle(cornerRadius: 6, style: .continuous)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// Per-model table under the chart: share bar plus the turn metrics.
private struct ModelUsageTable: View {
    let models: [ProviderTokenUsage]

    var body: some View {
        let total = max(models.reduce(UInt64(0)) { $0 + $1.totalTokens }, 1)
        VStack(spacing: 0) {
            header
            ForEach(Array(models.enumerated()), id: \.element.modelID) { index, usage in
                Divider()
                HStack(spacing: 12) {
                    Text(usage.modelID).lineLimit(1).truncationMode(.middle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    GeometryReader { geo in
                        Capsule().fill(usageSeriesColor(index).opacity(0.8))
                            .frame(
                                width: max(2, geo.size.width * Double(usage.totalTokens) / Double(total)),
                                height: 4
                            )
                            .frame(maxHeight: .infinity, alignment: .center)
                    }
                    .frame(width: 90)
                    metricCells(
                        formatTokenCount(usage.totalTokens),
                        "\(usage.requestCount)",
                        usage.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "—",
                        usage.averageTTFTMs.map { String(format: "%.0f ms", $0) } ?? "—",
                        usage.outputTPS.map(formatThroughput) ?? "—"
                    )
                }
                .font(.system(size: 12).monospacedDigit())
                .padding(.vertical, 8)
            }
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Text("模型").frame(maxWidth: .infinity, alignment: .leading)
            Text("占比").frame(width: 90, alignment: .leading)
            metricCells("Token", "请求", "缓存", "TTFT", "吞吐")
        }
        .font(.caption.weight(.medium))
        .foregroundStyle(.secondary)
        .padding(.bottom, 8)
    }

    @ViewBuilder
    private func metricCells(
        _ tokens: String, _ requests: String, _ cache: String, _ ttft: String, _ tps: String
    ) -> some View {
        Text(tokens).frame(width: 64, alignment: .trailing)
        Text(requests).frame(width: 52, alignment: .trailing)
        Text(cache).frame(width: 56, alignment: .trailing)
        Text(ttft).frame(width: 64, alignment: .trailing)
        Text(tps).frame(width: 52, alignment: .trailing)
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

private struct QuotaCard: View {
    let usage: ProviderQuotaUsage
    let multiple: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(providerQuotaLabel(usage, multiple: multiple))
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Text(providerQuotaText(usage))
                .font(.title3.weight(.semibold).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.6)
            if let used = usage.used, let limit = usage.limit, limit > 0 {
                ProgressView(value: min(max(used / limit, 0), 1))
                    .tint(.accentColor)
            } else {
                Color.clear.frame(height: 4)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            Color(nsColor: .controlBackgroundColor),
            in: RoundedRectangle(cornerRadius: 12, style: .continuous)
        )
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(.separator)
        }
        .help(usage.error ?? "")
    }
}

private struct UsageStatStrip: View {
    let group: ProviderUsageGroup

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            statCell(label: "Token", value: formatTokenCount(totalTokens), detail: tokenBreakdown)
            cellDivider
            statCell(label: "请求", value: "\(requestCount)", detail: nil)
            cellDivider
            cacheCell
            cellDivider
            statCell(label: "输出速度", value: speedValue, detail: ttftDetail)
        }
    }

    private var cellDivider: some View {
        Divider().frame(height: 46).padding(.horizontal, 10)
    }

    private func statCell(label: String, value: String, detail: String?) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label).font(.caption).foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 26, weight: .semibold).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.6)
            if let detail {
                Text(detail).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var cacheCell: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("缓存命中率").font(.caption).foregroundStyle(.secondary)
            Text(cacheHitValue)
                .font(.system(size: 26, weight: .semibold).monospacedDigit())
            if let ratio = cacheHitRatio {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.secondary.opacity(0.15))
                        Capsule().fill(Color.green).frame(width: geo.size.width * ratio)
                    }
                }
                .frame(height: 4)
            } else {
                Color.clear.frame(height: 4)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var totalTokens: UInt64 { group.models.reduce(0) { $0 + $1.totalTokens } }
    private var requestCount: UInt64 { group.models.reduce(0) { $0 + $1.requestCount } }
    private var totalInput: UInt64 { group.models.reduce(0) { $0 + $1.inputTokens } }
    private var totalOutput: UInt64 { group.models.reduce(0) { $0 + $1.outputTokens } }
    private var totalCacheRead: UInt64 { group.models.reduce(0) { $0 + $1.cacheReadTokens } }

    private var tokenBreakdown: String {
        "输入 \(formatTokenCount(totalInput)) · 输出 \(formatTokenCount(totalOutput)) · 缓存 \(formatTokenCount(totalCacheRead))"
    }

    private var cacheHitRatio: Double? {
        let base = totalInput + totalCacheRead
        guard base > 0 else { return nil }
        return Double(totalCacheRead) / Double(base)
    }

    private var cacheHitValue: String {
        cacheHitRatio.map { String(format: "%.1f%%", $0 * 100) } ?? "—"
    }

    private var speedValue: String {
        let weighted = group.models.compactMap { usage -> (Double, Double)? in
            guard let tps = usage.outputTPS, usage.outputTokens > 0 else { return nil }
            return (tps, Double(usage.outputTokens))
        }
        let weight = weighted.reduce(0) { $0 + $1.1 }
        guard weight > 0 else { return "—" }
        let average = weighted.reduce(0) { $0 + $1.0 * $1.1 } / weight
        return "\(Int(average.rounded())) token/秒"
    }

    private var ttftDetail: String? {
        let weighted = group.models.compactMap { usage -> (Double, Double)? in
            guard let ttft = usage.averageTTFTMs, usage.requestCount > 0 else { return nil }
            return (ttft, Double(usage.requestCount))
        }
        let weight = weighted.reduce(0) { $0 + $1.1 }
        guard weight > 0 else { return nil }
        let average = weighted.reduce(0) { $0 + $1.0 * $1.1 } / weight
        return average >= 1_000
            ? String(format: "首字平均 %.1f 秒", average / 1_000)
            : String(format: "首字平均 %.0f ms", average)
    }
}

/// Vertical bar chart of per-model token usage, mirroring magpie's model
/// breakdown: value label on top, proportional bar, model name below. Scrolls
/// horizontally once the bars overflow the window width.
private struct ModelUsageChart: View {
    let models: [ProviderTokenUsage]
    let selectedModelID: String?
    let onSelect: (String) -> Void

    // The chart shows the top models across the full width; the table below
    // lists every model, so the tail does not need horizontal scrolling.
    private let maxBars = 10
    private let chartHeight: CGFloat = 140

    var body: some View {
        let maximumTokens = models.map(\.totalTokens).max() ?? 0
        HStack(alignment: .bottom, spacing: 12) {
            ForEach(Array(models.prefix(maxBars).enumerated()), id: \.element.modelID) { index, usage in
                ModelUsageBar(
                    usage: usage,
                    seriesColor: usageSeriesColor(index),
                    maximumTokens: maximumTokens,
                    chartHeight: chartHeight,
                    selected: usage.modelID == selectedModelID,
                    onSelect: onSelect
                )
                .frame(maxWidth: 120)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct ModelUsageBar: View {
    let usage: ProviderTokenUsage
    let seriesColor: Color
    let maximumTokens: UInt64
    let chartHeight: CGFloat
    let selected: Bool
    let onSelect: (String) -> Void

    var body: some View {
        VStack(spacing: 6) {
            Text(formatTokenCount(usage.totalTokens))
                .font(.caption2.monospacedDigit().weight(.medium))
                .foregroundStyle(selected ? .primary : .secondary)
                .lineLimit(1)
                .fixedSize()
            RoundedRectangle(cornerRadius: 5, style: .continuous)
                .fill(seriesColor.opacity(selected ? 1 : 0.72))
                .frame(width: 30, height: barHeight)
                .frame(height: chartHeight, alignment: .bottom)
            Text(usage.modelID)
                .font(.caption2)
                .foregroundStyle(selected ? .primary : .secondary)
                .lineLimit(2)
                .multilineTextAlignment(.center)
                .frame(height: 30, alignment: .top)
        }
        .contentShape(Rectangle())
        .onTapGesture { onSelect(usage.modelID) }
        .help(usage.modelID)
        .accessibilityIdentifier("token-model-\(usage.modelID)")
    }

    private var barHeight: CGFloat {
        guard maximumTokens > 0 else { return 4 }
        let ratio = Double(usage.totalTokens) / Double(maximumTokens)
        return max(4, chartHeight * CGFloat(ratio))
    }
}

private func usageSeriesColor(_ index: Int) -> Color {
    let palette: [Color] = [.accentColor, .teal, .green, .orange, .purple, .pink, .indigo, .mint]
    return palette[index % palette.count]
}

private let reqColTime: CGFloat = 84
private let reqColClient: CGFloat = 96
private let reqColNum: CGFloat = 70
private let reqColTTFT: CGFloat = 68
private let reqColTPS: CGFloat = 86

private struct RequestUsageTable: View {
    let rows: [RequestUsageRow]

    var body: some View {
        // The page scrolls as a whole, so the table lays out inline.
        LazyVStack(alignment: .leading, spacing: 0) {
            requestHeader
            ForEach(rows) { row in
                Divider()
                RequestUsageRowView(row: row)
            }
        }
    }

    private var requestHeader: some View {
        HStack(spacing: 10) {
            Text("时间").frame(width: reqColTime, alignment: .leading)
            Text("客户端").frame(width: reqColClient, alignment: .leading)
            Text("模型").frame(maxWidth: .infinity, alignment: .leading)
            Text("输入").frame(width: reqColNum, alignment: .trailing)
            Text("缓存").frame(width: reqColNum, alignment: .trailing)
            Text("输出").frame(width: reqColNum, alignment: .trailing)
            Text("TTFT").frame(width: reqColTTFT, alignment: .trailing)
            Text("吞吐 t/s").frame(width: reqColTPS, alignment: .trailing)
        }
        .font(.caption.weight(.semibold))
        .foregroundStyle(.secondary)
        .padding(.vertical, 8)
    }
}

private struct RequestUsageRowView: View {
    let row: RequestUsageRow

    var body: some View {
        HStack(spacing: 10) {
            Text(requestTimeLabel(row.recordedAtMs))
                .foregroundStyle(.secondary)
                .frame(width: reqColTime, alignment: .leading)
            RequestClientBadge(clientID: row.clientID)
                .frame(width: reqColClient, alignment: .leading)
            Text(row.modelID)
                .frame(maxWidth: .infinity, alignment: .leading)
                .lineLimit(1)
                .truncationMode(.middle)
                .help("\(row.providerID)/\(row.modelID)")
            Text(formatTokenCount(row.inputTokens)).frame(width: reqColNum, alignment: .trailing)
            Text(formatTokenCount(row.cacheReadTokens))
                .foregroundStyle(row.cacheReadTokens > 0 ? Color.accentColor : Color.secondary)
                .frame(width: reqColNum, alignment: .trailing)
            Text(formatTokenCount(row.outputTokens)).frame(width: reqColNum, alignment: .trailing)
            Text(row.ttftMs.map { String(format: "%.0fms", $0) } ?? "—")
                .foregroundStyle(.secondary)
                .frame(width: reqColTTFT, alignment: .trailing)
            Text(row.outputTPS.map(formatThroughput) ?? "—")
                .frame(width: reqColTPS, alignment: .trailing)
        }
        .font(.system(size: 12).monospacedDigit())
        .padding(.vertical, 8)
        .help(row.prefixState.map { "缓存状态：\($0)" } ?? "")
    }
}

private struct RequestClientBadge: View {
    let clientID: String

    var body: some View {
        Text(clientDisplayLabel(clientID))
            .font(.caption2.weight(.medium))
            .lineLimit(1)
            .truncationMode(.tail)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(clientBadgeColor(clientID).opacity(0.16), in: Capsule())
            .foregroundStyle(clientBadgeColor(clientID))
    }
}

private func clientDisplayLabel(_ clientID: String) -> String {
    let trimmed = clientID.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty || trimmed.lowercased() == "unknown" { return "未知" }
    return trimmed
}

private func clientBadgeColor(_ clientID: String) -> Color {
    switch clientID.lowercased() {
    case "codex": return Color(red: 0.10, green: 0.68, blue: 0.56)
    case "claude": return Color(red: 0.80, green: 0.45, blue: 0.18)
    case "dsh", "deepseek": return Color(red: 0.34, green: 0.53, blue: 1)
    case "opencode": return Color(red: 0.42, green: 0.44, blue: 0.95)
    case "pi": return Color(red: 0.85, green: 0.33, blue: 0.57)
    default: return .secondary
    }
}

private func formatThroughput(_ value: Double) -> String {
    if value >= 1_000 { return String(format: "%.1fk", value / 1_000) }
    if value >= 100 { return String(format: "%.0f", value) }
    return String(format: "%.1f", value)
}

private func requestTimeLabel(_ recordedAtMs: UInt64) -> String {
    let date = Date(timeIntervalSince1970: Double(recordedAtMs) / 1_000.0)
    let formatter = DateFormatter()
    formatter.dateFormat = "MM-dd HH:mm"
    return formatter.string(from: date)
}

private struct TokenModelDetail: View {
    let usage: ProviderTokenUsage

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(usage.modelID)
                .font(.caption2.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)
            Grid(horizontalSpacing: 6, verticalSpacing: 6) {
                GridRow {
                    metric("请求", "\(usage.requestCount)")
                    metric("输入", formatTokenCount(usage.inputTokens))
                    metric("缓存输入", formatTokenCount(usage.cacheReadTokens))
                    metric("输出", formatTokenCount(usage.outputTokens))
                }
                GridRow {
                    metric("缓存比例", usage.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "未上报")
                    metric("缓存输出", formatTokenCount(usage.cacheCreationTokens))
                    metric("首字响应", usage.averageTTFTMs.map { String(format: "%.0f ms", $0) } ?? "未上报")
                    metric("每秒吞吐", usage.outputTPS.map { String(format: "%.1f tok/s", $0) } ?? "未上报")
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(
            Color(nsColor: .controlBackgroundColor),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .overlay {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(.separator)
        }
    }

    private func metric(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.system(size: 8)).foregroundStyle(.secondary)
            Text(value).font(.system(size: 10, weight: .semibold).monospacedDigit())
                .lineLimit(1)
                .frame(minWidth: 58, alignment: .leading)
        }
    }
}

final class ProviderUsageDashboardView: FlippedMenuView {
    let model = ProviderUsageDashboardModel()
    private let hostingView: NSHostingView<ProviderUsageDashboardContent>
    /// Tracks whether a coalesced row-height sync is already queued so repeated
    /// data refreshes collapse into a single deferred resize.
    private var heightSyncScheduled = false

    init() {
        hostingView = NSHostingView(rootView: ProviderUsageDashboardContent(model: model, compact: true))
        super.init(frame: NSRect(x: 0, y: 0, width: menuContentWidth, height: providerDashboardMinimumHeight))
        installHostingView()
    }

    required init?(coder: NSCoder) {
        hostingView = NSHostingView(rootView: ProviderUsageDashboardContent(model: model, compact: true))
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
        hostingView.rootView = ProviderUsageDashboardContent(model: model, compact: true)
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        model.quotaUsages = usages
        model.quotaStatusTitle = L10n.Provider.quotaEmpty
        model.quotaStatusDetail = nil
        model.normalizeSelection()
        scheduleContentHeightUpdate()
    }

    func updateTokenStatus(title: String, detail: String?) {
        model.tokenStatusTitle = title
        model.tokenStatusDetail = detail
        scheduleContentHeightUpdate()
    }

    func updateTokenUsages(_ usages: [ProviderTokenUsage]) {
        model.tokenUsages = usages
        model.tokenStatusTitle = "Token 使用：暂无数据"
        model.tokenStatusDetail = nil
        model.normalizeSelection()
        scheduleContentHeightUpdate()
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

final class ProviderUsageWindowController: NSWindowController {
    let model: ProviderUsageDashboardModel
    private let hostingController: NSHostingController<ProviderUsageDashboardContent>

    init() {
        let model = ProviderUsageDashboardModel()
        self.model = model
        hostingController = NSHostingController(
            rootView: ProviderUsageDashboardContent(model: model, compact: false)
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 620),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "用量与会话"
        window.minSize = NSSize(width: 760, height: 520)
        window.toolbarStyle = .unified
        configureOpaqueWindow(window)
        window.contentViewController = hostingController
        window.center()
        super.init(window: window)
        configurePersistentWindow(window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        if let window {
            presentPersistentWindow(window)
        }
        model.onRequestRefresh?()
    }

    func updateQuotaStatus(title: String, detail: String?) {
        model.quotaStatusTitle = title
        model.quotaStatusDetail = detail
    }

    func updateConfiguredProviders(_ providers: [ProviderDashboardProvider]) {
        model.configuredProviders = providers
        model.normalizeSelection()
    }

    func refreshProviderIcons() {
        hostingController.rootView = ProviderUsageDashboardContent(model: model, compact: false)
    }

    func updateQuotaUsages(_ usages: [ProviderQuotaUsage]) {
        model.quotaUsages = usages
        model.quotaStatusTitle = L10n.Provider.quotaEmpty
        model.quotaStatusDetail = nil
        model.normalizeSelection()
    }

    func updateTokenStatus(title: String, detail: String?) {
        model.tokenStatusTitle = title
        model.tokenStatusDetail = detail
    }

    func updateTokenUsages(_ usages: [ProviderTokenUsage]) {
        model.tokenUsages = usages
        model.tokenStatusTitle = "Token 使用：暂无数据"
        model.tokenStatusDetail = nil
        model.normalizeSelection()
    }

    var onRangeChange: ((TokenUsageRange) -> Void)? {
        get { model.onRangeChange }
        set { model.onRangeChange = newValue }
    }

    var onRequestRefresh: (() -> Void)? {
        get { model.onRequestRefresh }
        set { model.onRequestRefresh = newValue }
    }

    func updateRequestRows(_ rows: [RequestUsageRow]) {
        model.requestRows = rows
        model.requestStatus = "请求明细：暂无数据"
    }

    func updateRequestStatus(_ status: String) {
        model.requestStatus = status
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
