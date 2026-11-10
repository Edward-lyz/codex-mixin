import Charts
import SwiftUI

enum ConsoleUsageRange: Int, CaseIterable, Identifiable {
    case day = 1
    case week = 7
    case month = 30

    var id: Int { rawValue }
    var title: String { AppLocalization.string("console.usage.days", rawValue) }

    /// The one-day chart is drawn per hour from reply rows, so it asks for
    /// the gateway's whole reply ledger; longer ranges only list replies.
    var turnLimit: Int { self == .day ? 5_000 : 500 }
    var chartBucket: UsageChartBucket { self == .day ? .hour : .day }
}

enum UsageStatusFilter: Hashable {
    case all
    case succeeded
    case failed
}

@MainActor
final class ConsoleUsageModel: ObservableObject {
    /// `days` asks for daily rows covering the range and the equal-length
    /// period before it, so every card can compare with the previous period.
    typealias Loader = (_ days: Int, _ turnLimit: Int) async throws -> (
        [DailyUsageRecord], [TurnUsageRecord], [String: String], [String: String]
    )
    /// Replies inside one chart slot, read straight from the gateway ledger.
    typealias SlotLoader = (_ interval: DateInterval) async throws -> [TurnUsageRecord]

    @Published var range = ConsoleUsageRange.week
    @Published var splitByProvider = false
    @Published var providerFilter: String?
    @Published var sessionFilter: String?
    @Published var statusFilter = UsageStatusFilter.all
    @Published var modelFilter: String?
    @Published var substitutedOnly = false
    /// A chart slot the user clicked; the reply table lists only its replies.
    @Published var slotFilter: UsageSlot? {
        didSet {
            guard slotFilter?.interval != oldValue?.interval else { return }
            loadSlotTurns()
        }
    }
    /// Replies of the selected slot; nil while none is selected or loading.
    @Published private(set) var slotTurns: [TurnUsageRecord]?
    @Published private(set) var isLoadingSlot = false
    @Published private(set) var daily: [DailyUsageRecord] = []
    @Published private(set) var previousDaily: [DailyUsageRecord] = []
    @Published private(set) var turns: [TurnUsageRecord] = []
    @Published private(set) var providerNames: [String: String] = [:]
    /// Provider website by provider id, used to find each provider's logo.
    @Published private(set) var providerWebsites: [String: String] = [:]
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var turnsTruncated = false
    private let loader: Loader
    private let slotLoader: SlotLoader
    private var slotTask: Task<Void, Never>?

    init(loader: @escaping Loader, slotLoader: @escaping SlotLoader) {
        self.loader = loader
        self.slotLoader = slotLoader
    }

    var providers: [String] {
        Array(Set(daily.map(\.providerID) + turns.map(\.providerID))).sorted()
    }

    var filteredDaily: [DailyUsageRecord] {
        daily.filter { providerFilter == nil || $0.providerID == providerFilter }
    }

    var filteredPreviousDaily: [DailyUsageRecord] {
        previousDaily.filter { providerFilter == nil || $0.providerID == providerFilter }
    }

    /// Replies for the chart: provider filter only, so a clicked slot or
    /// session never empties the other bars.
    var chartTurns: [TurnUsageRecord] {
        turns.filter { providerFilter == nil || $0.providerID == providerFilter }
    }

    /// Replies for the table before the status/model/substitution filters,
    /// which the filter bar counts against.
    var scopedTurns: [TurnUsageRecord] {
        let source = slotFilter == nil ? chartTurns : (slotTurns ?? [])
        return source.filter { turn in
            (providerFilter == nil || turn.providerID == providerFilter)
                && (sessionFilter == nil || turn.session == sessionFilter)
                && (slotFilter.map { $0.interval.start <= turn.date && turn.date < $0.interval.end } ?? true)
        }
    }

    var filteredTurns: [TurnUsageRecord] {
        scopedTurns.filter { turn in
            switch statusFilter {
            case .all: break
            case .succeeded: if turn.isFailed { return false }
            case .failed: if !turn.isFailed { return false }
            }
            return (modelFilter == nil || turn.upstreamModelID == modelFilter)
                && (!substitutedOnly || turn.isSubstituted)
        }
    }

    /// The ranged reply list holds only the newest replies, so a clicked bar
    /// asks the ledger for exactly its own interval.
    private func loadSlotTurns() {
        slotTask?.cancel()
        slotTurns = nil
        guard let interval = slotFilter?.interval else {
            isLoadingSlot = false
            return
        }
        isLoadingSlot = true
        slotTask = Task { [weak self] in
            guard let self else { return }
            let loaded = try? await slotLoader(interval)
            guard !Task.isCancelled, slotFilter?.interval == interval else { return }
            slotTurns = loaded ?? []
            isLoadingSlot = false
        }
    }

    private var loadGeneration = 0

    /// Starts a load for the current range. A newer load supersedes older
    /// ones, so switching range mid-load never shows the old range's rows.
    func refresh() async {
        loadGeneration += 1
        let generation = loadGeneration
        let range = range
        isLoading = true
        do {
            let (rows, loadedTurns, names, websites) = try await loader(range.rawValue * 2, range.turnLimit)
            guard generation == loadGeneration else { return }
            let firstDay = currentUnixDay() - range.rawValue + 1
            daily = rows.filter { $0.day >= firstDay }
            previousDaily = rows.filter { $0.day < firstDay }
            turns = loadedTurns.filter { $0.atMs >= UInt64(max(firstDay, 0)) * 86_400_000 }
            providerNames = names
            providerWebsites = websites
            turnsTruncated = loadedTurns.count >= range.turnLimit
            if slotFilter != nil { loadSlotTurns() }
            if let providerFilter, !providers.contains(providerFilter) {
                self.providerFilter = nil
                sessionFilter = nil
            }
            if let modelFilter, !turns.contains(where: { $0.upstreamModelID == modelFilter }) {
                self.modelFilter = nil
            }
            errorMessage = nil
        } catch {
            guard generation == loadGeneration else { return }
            errorMessage = error.localizedDescription
        }
        isLoading = false
    }

    func selectRange(_ range: ConsoleUsageRange) {
        guard range != self.range else { return }
        self.range = range
        sessionFilter = nil
        slotFilter = nil
        Task { await refresh() }
    }
}

private let usagePageSize = 50
private let usageSeriesColors: [Color] = [.blue, .orange, .teal, .purple, .pink, .green, .indigo, .yellow, .mint, .red]
private let usageCachedColor = Color.teal
private let usageUncachedColor = Color.blue
private let usageOutputColor = Color.purple
private let usageFailedColor = Color.red

/// One table row: a reply, or a session with its replies as children.
private struct UsageTableRow: Identifiable {
    let id: String
    let turn: TurnUsageRecord?
    let session: String
    let sessionTitle: String?
    let date: Date
    let stats: UsageSlotStats
    let models: Set<String>
    /// Replies of a session row; nil for reply rows.
    var children: [UsageTableRow]?
    /// Reply listed under its expanded session row.
    var isNested = false
}

/// One stacked bar segment.
private struct UsageBar: Identifiable {
    let slot: UsageSlot
    let series: String
    let value: UInt64
    var id: String { "\(slot.id)-\(series)" }
}

struct ConsoleUsageView: View {
    @ObservedObject var model: ConsoleUsageModel
    @State private var pageIndex = 0
    @State private var chartShowsRequests = false
    @State private var groupBySession = false
    @State private var expandedSessions: Set<String> = []
    @State private var hoveredSlot: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            toolbar
            if let message = model.errorMessage {
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .font(.callout)
                    .foregroundStyle(.red)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            summary
            chartCard
            turnsCard
                .frame(maxHeight: .infinity)
        }
        .padding(20)
        .frame(minWidth: 860, minHeight: 620, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
        .onChange(of: model.range) { _ in pageIndex = 0 }
        .onChange(of: model.providerFilter) { _ in pageIndex = 0 }
        .onChange(of: model.sessionFilter) { _ in pageIndex = 0 }
        .onChange(of: model.slotFilter) { _ in pageIndex = 0 }
        .onChange(of: model.statusFilter) { _ in pageIndex = 0 }
        .onChange(of: model.modelFilter) { _ in pageIndex = 0 }
        .onChange(of: model.substitutedOnly) { _ in pageIndex = 0 }
        .onChange(of: groupBySession) { _ in pageIndex = 0 }
    }

    // MARK: Toolbar

    private var toolbar: some View {
        HStack(spacing: 10) {
            Picker(AppLocalization.string("console.usage.range"), selection: Binding(
                get: { model.range },
                set: model.selectRange
            )) {
                ForEach(ConsoleUsageRange.allCases) { Text($0.title).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            Picker(AppLocalization.string("console.usage.provider"), selection: Binding(
                get: { model.providerFilter },
                set: { model.providerFilter = $0; model.sessionFilter = nil }
            )) {
                Text(AppLocalization.string("console.usage.allProviders")).tag(String?.none)
                Divider()
                ForEach(model.providers, id: \.self) { provider in
                    Text(model.providerNames[provider] ?? provider).tag(String?.some(provider))
                }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .fixedSize()
            Spacer()
            if model.isLoading { ProgressView().controlSize(.small) }
            Button {
                Task { await model.refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .help(AppLocalization.string("agentManagement.refresh"))
            .accessibilityLabel(AppLocalization.string("agentManagement.refresh"))
            .keyboardShortcut("r", modifiers: .command)
            .disabled(model.isLoading)
        }
    }

    private func filterChip(_ title: String, clear: @escaping () -> Void) -> some View {
        Button(action: clear) {
            HStack(spacing: 4) {
                Text(title).lineLimit(1).truncationMode(.middle)
                Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
            }
            .font(.caption)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .frame(maxWidth: 220)
            .background(Color.accentColor.opacity(0.14), in: Capsule())
        }
        .buttonStyle(.plain)
        .help(AppLocalization.string("console.usage.clearFilter"))
        .accessibilityLabel("\(AppLocalization.string("console.usage.clearFilter")): \(title)")
    }

    // MARK: Summary

    /// Four cards over the same population: every reply attempt in the range
    /// from the CLI's daily totals, each compared with the previous period.
    private var summary: some View {
        let current = UsageSlotStats(model.filteredDaily.map(\.stats))
        let previous = UsageSlotStats(model.filteredPreviousDaily.map(\.stats))
        let hasPrevious = previous.attempts > 0
        let slots = chartSlots
        return HStack(spacing: 12) {
            UsageKPICard(
                title: AppLocalization.string("console.usage.requests"),
                value: formatTokenCount(current.attempts),
                delta: hasPrevious ? relativeDelta(current.attempts, previous.attempts) : nil,
                deltaTone: .neutral,
                symbol: "arrow.up.arrow.down",
                tint: .blue,
                trend: slots.map { Double($0.total.attempts) }
            ) {
                Text(current.errors > 0
                    ? AppLocalization.string("console.usage.failedDetail", formatTokenCount(current.errors))
                    : " ")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            UsageKPICard(
                title: "Tokens",
                value: formatTokenCount(current.tokens),
                delta: hasPrevious ? relativeDelta(current.tokens, previous.tokens) : nil,
                deltaTone: .neutral,
                symbol: "square.stack.3d.up",
                tint: .purple,
                trend: slots.map { Double($0.total.tokens) }
            ) {
                tokenShareBar(current)
            }
            UsageKPICard(
                title: AppLocalization.string("console.usage.cacheHit"),
                value: current.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "—",
                delta: hasPrevious ? pointDelta(current.cacheHitPercent, previous.cacheHitPercent) : nil,
                deltaTone: .higherIsBetter,
                symbol: "bolt",
                tint: .green,
                trend: slots.compactMap(\.total.cacheHitPercent)
            ) {
                Text(current.cacheHitPercent == nil
                    ? " "
                    : AppLocalization.string("console.usage.cachedDetail", formatTokenCount(current.hitRead)))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            UsageKPICard(
                title: AppLocalization.string("console.usage.errorRate"),
                value: current.errorPercent.map { String(format: "%.1f%%", $0) } ?? "—",
                delta: hasPrevious ? pointDelta(current.errorPercent, previous.errorPercent) : nil,
                deltaTone: .lowerIsBetter,
                symbol: "exclamationmark.triangle",
                tint: .orange,
                trend: slots.map { $0.total.errorPercent ?? 0 },
                warning: (current.errorPercent ?? 0) >= 5
            ) {
                Text(AppLocalization.string("console.usage.failedOfAttempts",
                                            formatTokenCount(current.errors), formatTokenCount(current.attempts)))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
    }

    private func relativeDelta(_ current: UInt64, _ previous: UInt64) -> (text: String, sign: Int)? {
        guard previous > 0 else { return nil }
        let change = (Double(current) - Double(previous)) / Double(previous) * 100
        return (String(format: "%+.0f%%", change), change > 0.5 ? 1 : change < -0.5 ? -1 : 0)
    }

    private func pointDelta(_ current: Double?, _ previous: Double?) -> (text: String, sign: Int)? {
        guard let current, let previous else { return nil }
        let change = current - previous
        return (String(format: "%+.1f pp", change), change > 0.05 ? 1 : change < -0.05 ? -1 : 0)
    }

    private func tokenShareBar(_ stats: UsageSlotStats) -> some View {
        let total = max(Double(stats.tokens), 1)
        let parts: [(UInt64, Color)] = [
            (stats.cached, usageCachedColor),
            (stats.uncached, usageUncachedColor),
            (stats.output, usageOutputColor),
        ]
        let percent = { (value: UInt64) in String(format: "%.0f%%", Double(value) / total * 100) }
        return VStack(alignment: .leading, spacing: 3) {
            GeometryReader { geometry in
                HStack(spacing: 1) {
                    ForEach(parts.indices, id: \.self) { index in
                        Rectangle()
                            .fill(parts[index].1)
                            .frame(width: max(0, geometry.size.width * Double(parts[index].0) / total - 1))
                    }
                }
            }
            .frame(height: 5)
            .clipShape(Capsule())
            .background(Capsule().fill(Color.secondary.opacity(0.15)))
            Text(AppLocalization.string("console.usage.tokenShare",
                                        percent(stats.cached), percent(stats.uncached), percent(stats.output)))
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: Chart

    private var chartSlots: [UsageSlot] {
        usageSlots(
            daily: model.filteredDaily,
            turns: model.chartTurns,
            bucket: model.range.chartBucket,
            lastDay: currentUnixDay(),
            days: model.range.rawValue
        )
    }

    private var cachedSeries: String { AppLocalization.string("console.usage.series.cached") }
    private var uncachedSeries: String { AppLocalization.string("console.usage.series.uncached") }
    private var outputSeries: String { AppLocalization.string("console.usage.series.output") }
    private var succeededSeries: String { AppLocalization.string("console.usage.series.succeeded") }
    private var failedSeries: String { AppLocalization.string("console.usage.series.failed") }

    /// Bars in stacking order. The default stack shows how tokens split into
    /// cached input, uncached input and output (or replies into succeeded
    /// and failed); the provider grouping stacks providers instead.
    private func bars(_ slots: [UsageSlot]) -> (bars: [UsageBar], domain: [String], colors: [Color]) {
        if model.splitByProvider {
            let names = Array(Set(slots.flatMap { $0.byProvider.keys }.map(seriesName))).sorted()
            let bars = slots.flatMap { slot in
                slot.byProvider.sorted { seriesName($0.key) < seriesName($1.key) }.map { provider, stats in
                    UsageBar(slot: slot, series: seriesName(provider),
                             value: chartShowsRequests ? stats.attempts : stats.tokens)
                }
            }
            return (bars, names, names.indices.map { usageSeriesColors[$0 % usageSeriesColors.count] })
        }
        if chartShowsRequests {
            let bars = slots.flatMap { slot in
                [UsageBar(slot: slot, series: succeededSeries, value: slot.total.requests),
                 UsageBar(slot: slot, series: failedSeries, value: slot.total.errors)]
            }
            return (bars, [succeededSeries, failedSeries], [Color.accentColor, usageFailedColor])
        }
        let bars = slots.flatMap { slot in
            [UsageBar(slot: slot, series: cachedSeries, value: slot.total.cached),
             UsageBar(slot: slot, series: uncachedSeries, value: slot.total.uncached),
             UsageBar(slot: slot, series: outputSeries, value: slot.total.output)]
        }
        return (bars, [cachedSeries, uncachedSeries, outputSeries], [usageCachedColor, usageUncachedColor, usageOutputColor])
    }

    private var chartCard: some View {
        let bucket = model.range.chartBucket
        let slots = chartSlots
        let (bars, domain, colors) = bars(slots)
        let starts = slots.map(\.start)
        let highlighted = hoveredSlot ?? model.slotFilter?.start
        let isEmpty = bars.allSatisfy { $0.value == 0 }
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                Picker(AppLocalization.string("console.usage.metric"), selection: $chartShowsRequests) {
                    Text("Tokens").tag(false)
                    Text(AppLocalization.string("console.usage.requests")).tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .controlSize(.small)
                Picker(AppLocalization.string("console.usage.grouping"), selection: $model.splitByProvider) {
                    Text(AppLocalization.string("console.usage.composition")).tag(false)
                    Text(AppLocalization.string("console.usage.split")).tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .controlSize(.small)
                if bucket == .hour, model.turnsTruncated {
                    Text(AppLocalization.string("console.usage.truncated", model.range.turnLimit))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                chartLegend(domain: domain, colors: colors)
            }
            Chart(bars) { bar in
                BarMark(
                    x: .value("Time", bar.slot.start, unit: bucket.component),
                    y: .value("Value", bar.value)
                )
                .foregroundStyle(by: .value("Series", bar.series))
                .opacity(highlighted == nil || highlighted == bar.slot.start ? 1 : 0.35)
            }
            .chartForegroundStyleScale(domain: domain, range: colors)
            .chartXScale(domain: chartDomain(starts, bucket: bucket))
            .chartXAxis { xAxis(bucket: bucket) }
            .chartYAxis {
                AxisMarks(position: .leading, values: .automatic(desiredCount: 3)) {
                    AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [3, 3]))
                    AxisValueLabel(format: FloatingPointFormatStyle<Double>(locale: Locale(identifier: "en_US"))
                        .notation(.compactName).precision(.fractionLength(0...1)))
                }
            }
            .chartLegend(.hidden)
            .chartOverlay { proxy in
                chartInteraction(proxy: proxy, slots: slots, bucket: bucket)
            }
            .overlay {
                if isEmpty, !model.isLoading {
                    Text(AppLocalization.string("console.usage.noData"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(12)
        .frame(height: 170)
        .background(ConsoleCardBackground())
    }

    private func chartLegend(domain: [String], colors: [Color]) -> some View {
        HStack(spacing: 10) {
            ForEach(Array(domain.prefix(6).enumerated()), id: \.offset) { index, name in
                HStack(spacing: 4) {
                    RoundedRectangle(cornerRadius: 2).fill(colors[index]).frame(width: 8, height: 8)
                    Text(name).lineLimit(1)
                }
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    private func seriesName(_ series: String) -> String {
        model.providerNames[series] ?? series
    }

    private func chartDomain(_ starts: [Date], bucket: UsageChartBucket) -> ClosedRange<Date> {
        let first = starts.first ?? Date()
        let last = starts.last ?? first
        return first...(Calendar.current.date(byAdding: bucket.component, value: 1, to: last) ?? last)
    }

    private func xAxis(bucket: UsageChartBucket) -> some AxisContent {
        let stride: Int
        switch bucket {
        case .hour: stride = 3
        case .day: stride = model.range == .month ? 5 : 1
        }
        return AxisMarks(values: .stride(by: bucket.component, count: stride)) { _ in
            AxisTick()
            switch bucket {
            case .hour:
                AxisValueLabel(format: .dateTime.hour(), centered: true)
            case .day:
                AxisValueLabel(format: .dateTime.month(.defaultDigits).day(), centered: true)
            }
        }
    }

    /// Hover shows the slot's breakdown beside the bar; a click lists only
    /// that slot's replies in the table, and a second click clears it.
    private func chartInteraction(proxy: ChartProxy, slots: [UsageSlot], bucket: UsageChartBucket) -> some View {
        let starts = slots.map(\.start)
        return GeometryReader { geometry in
            let plot = geometry[proxy.plotAreaFrame]
            ZStack(alignment: .topLeading) {
                Rectangle()
                    .fill(.clear)
                    .contentShape(Rectangle())
                    .onContinuousHover { phase in
                        guard case .active(let location) = phase,
                              plot.contains(location),
                              let date: Date = proxy.value(atX: location.x - plot.minX)
                        else {
                            hoveredSlot = nil
                            return
                        }
                        hoveredSlot = starts.last { $0 <= date }
                    }
                    .onTapGesture {
                        guard let hoveredSlot, let slot = slots.first(where: { $0.start == hoveredSlot }) else { return }
                        model.slotFilter = model.slotFilter?.start == hoveredSlot ? nil : slot
                    }
                if let hoveredSlot,
                   let slot = slots.first(where: { $0.start == hoveredSlot }),
                   let center = Calendar.current.date(byAdding: bucket.component, value: 1, to: hoveredSlot)
                    .map({ hoveredSlot.addingTimeInterval($0.timeIntervalSince(hoveredSlot) / 2) }),
                   let x = proxy.position(forX: center).map({ $0 + plot.minX }) {
                    let leftHalf = x < geometry.size.width / 2
                    HStack(spacing: 0) {
                        if !leftHalf { Spacer(minLength: 0) }
                        tooltip(slot)
                            .padding(leftHalf ? .leading : .trailing, leftHalf ? x + 12 : geometry.size.width - x + 12)
                        if leftHalf { Spacer(minLength: 0) }
                    }
                    .frame(width: geometry.size.width)
                    .padding(.top, plot.minY)
                    .allowsHitTesting(false)
                }
            }
        }
    }

    private func tooltip(_ slot: UsageSlot) -> some View {
        let stats = slot.total
        return VStack(alignment: .leading, spacing: 5) {
            Text(slotTitle(slot.start)).font(.callout.weight(.semibold))
            tooltipRow(AppLocalization.string("console.usage.requests"), formatTokenCount(stats.attempts), nil)
            if stats.errors > 0 {
                tooltipRow(failedSeries, formatTokenCount(stats.errors), usageFailedColor)
            }
            Divider()
            tooltipRow(cachedSeries, formatTokenCount(stats.cached), usageCachedColor)
            tooltipRow(uncachedSeries, formatTokenCount(stats.uncached), usageUncachedColor)
            tooltipRow(outputSeries, formatTokenCount(stats.output), usageOutputColor)
            if let hit = stats.cacheHitPercent {
                tooltipRow(AppLocalization.string("console.usage.cacheHit"), String(format: "%.1f%%", hit), nil)
            }
            if model.splitByProvider, slot.byProvider.count > 1 {
                Divider()
                ForEach(slot.byProvider.sorted { $0.value.tokens > $1.value.tokens }, id: \.key) { provider, stats in
                    tooltipRow(seriesName(provider),
                               formatTokenCount(chartShowsRequests ? stats.attempts : stats.tokens), nil)
                }
            }
            Text(AppLocalization.string("console.usage.slotHelp"))
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .frame(minWidth: 200, alignment: .leading)
        .fixedSize()
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor).opacity(0.6))
        )
        .shadow(color: .black.opacity(0.12), radius: 8, y: 2)
    }

    private func tooltipRow(_ title: String, _ value: String, _ color: Color?) -> some View {
        HStack(spacing: 6) {
            if let color {
                RoundedRectangle(cornerRadius: 2).fill(color).frame(width: 7, height: 7)
            }
            Text(title).lineLimit(1)
            Spacer(minLength: 14)
            Text(value).monospacedDigit()
        }
        .font(.caption)
    }

    private func slotTitle(_ start: Date) -> String {
        switch model.range.chartBucket {
        case .hour:
            let end = start.addingTimeInterval(3_600)
            let time = Date.FormatStyle().hour(.twoDigits(amPM: .omitted)).minute()
            return "\(start.formatted(.dateTime.month(.defaultDigits).day())) "
                + "\(start.formatted(time))–\(end.formatted(time))"
        case .day:
            return start.formatted(.dateTime.month(.defaultDigits).day().weekday(.abbreviated))
        }
    }

    // MARK: Replies

    private var tableFilters: some View {
        let scoped = model.scopedTurns
        let failed = scoped.filter(\.isFailed).count
        let substituted = scoped.filter(\.isSubstituted).count
        let models = Array(Set(scoped.map(\.upstreamModelID))).sorted()
        return HStack(spacing: 10) {
            Picker(AppLocalization.string("console.usage.status"), selection: $model.statusFilter) {
                Text(AppLocalization.string("console.usage.statusAll")).tag(UsageStatusFilter.all)
                Text(succeededSeries).tag(UsageStatusFilter.succeeded)
                Text(failed > 0 ? "\(failedSeries) \(failed)" : failedSeries).tag(UsageStatusFilter.failed)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            .controlSize(.small)
            Picker(AppLocalization.string("console.usage.model"), selection: $model.modelFilter) {
                Text(AppLocalization.string("console.usage.allModels")).tag(String?.none)
                Divider()
                ForEach(models, id: \.self) { Text($0).tag(String?.some($0)) }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .fixedSize()
            .controlSize(.small)
            if substituted > 0 || model.substitutedOnly {
                Toggle(AppLocalization.string("console.usage.substitutedOnly", substituted), isOn: $model.substitutedOnly)
                    .toggleStyle(.checkbox)
                    .foregroundStyle(.orange)
                    .help(AppLocalization.string("console.usage.substitutedHelp"))
            }
            Toggle(AppLocalization.string("console.usage.groupBySession"), isOn: $groupBySession)
                .toggleStyle(.checkbox)
            if let session = model.sessionFilter {
                filterChip(model.turns.first { $0.session == session }?.sessionDisplay ?? session) {
                    model.sessionFilter = nil
                }
            }
            if let slot = model.slotFilter {
                filterChip(slotTitle(slot.start)) { model.slotFilter = nil }
            }
            Spacer(minLength: 0)
        }
        .font(.callout)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    private func tableRows(_ turns: [TurnUsageRecord]) -> [UsageTableRow] {
        let replyRow = { (turn: TurnUsageRecord) in
            UsageTableRow(id: turn.id, turn: turn, session: turn.session, sessionTitle: turn.sessionDisplay,
                          date: turn.date, stats: turn.stats, models: [turn.upstreamModelID], children: nil)
        }
        guard groupBySession else { return turns.map(replyRow) }
        var order: [String] = []
        var grouped: [String: [TurnUsageRecord]] = [:]
        for turn in turns {
            if grouped[turn.session] == nil { order.append(turn.session) }
            grouped[turn.session, default: []].append(turn)
        }
        return order.compactMap { session in
            guard let replies = grouped[session], let latest = replies.first else { return nil }
            return UsageTableRow(
                id: "session-\(session)",
                turn: nil,
                session: session,
                sessionTitle: latest.sessionDisplay,
                date: latest.date,
                stats: UsageSlotStats(replies.map(\.stats)),
                models: Set(replies.map(\.upstreamModelID)),
                children: replies.map(replyRow)
            )
        }
    }

    /// Session rows followed by their replies when expanded. A flat table
    /// keeps macOS 13 support; hierarchical tables need macOS 14.
    private func flattened(_ rows: [UsageTableRow]) -> [UsageTableRow] {
        rows.flatMap { row -> [UsageTableRow] in
            guard let children = row.children, expandedSessions.contains(row.session) else { return [row] }
            return [row] + children.map { child in
                var nested = child
                nested.isNested = true
                return nested
            }
        }
    }

    private var turnsCard: some View {
        let rows = tableRows(model.filteredTurns)
        let pageCount = max(1, (rows.count + usagePageSize - 1) / usagePageSize)
        let page = min(pageIndex, pageCount - 1)
        return VStack(spacing: 0) {
            tableFilters
            Divider()
            turnsTable(flattened(Array(rows.dropFirst(page * usagePageSize).prefix(usagePageSize))))
                .overlay {
                    if model.isLoadingSlot {
                        ProgressView().controlSize(.small)
                    } else if rows.isEmpty, model.slotFilter != nil {
                        Text(AppLocalization.string("console.usage.noSlotReplies"))
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .padding()
                    }
                }
            Divider()
            HStack(spacing: 8) {
                Text(AppLocalization.string("console.usage.replyCount", model.filteredTurns.count))
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    pageIndex = page - 1
                } label: {
                    Image(systemName: "chevron.left")
                }
                .disabled(page == 0)
                .help(AppLocalization.string("console.usage.previousPage"))
                .accessibilityLabel(AppLocalization.string("console.usage.previousPage"))
                Text("\(page + 1) / \(pageCount)").monospacedDigit()
                Button {
                    pageIndex = page + 1
                } label: {
                    Image(systemName: "chevron.right")
                }
                .disabled(page + 1 >= pageCount)
                .help(AppLocalization.string("console.usage.nextPage"))
                .accessibilityLabel(AppLocalization.string("console.usage.nextPage"))
            }
            .buttonStyle(.borderless)
            .font(.caption)
            .padding(.horizontal, 12)
            .padding(.vertical, 5)
        }
        .background(ConsoleCardBackground())
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private func turnsTable(_ rows: [UsageTableRow]) -> some View {
        Table(rows) {
            TableColumn(AppLocalization.string("console.usage.status")) { row in
                statusCell(row)
            }
            .width(min: 34, ideal: 40, max: 70)
            TableColumn(AppLocalization.string("console.usage.time")) { row in
                Text(timeText(row.date)).monospacedDigit()
            }
            .width(min: 64, ideal: model.range == .day ? 72 : 110)
            TableColumn(AppLocalization.string("console.usage.session")) { row in
                sessionCell(row)
            }
            .width(min: 110, ideal: 180)
            TableColumn(AppLocalization.string("console.usage.model")) { row in
                modelCell(row)
            }
            .width(min: 160, ideal: 230)
            TableColumn(AppLocalization.string("console.usage.uncached")) { row in
                Text(row.turn.map { $0.inputTokens.map(formatTokenCount) ?? "—" } ?? formatTokenCount(row.stats.uncached))
                    .monospacedDigit()
            }
            .width(min: 54, ideal: 70)
            TableColumn(AppLocalization.string("console.usage.cached")) { row in
                Text(cachedText(row))
                    .monospacedDigit()
                    .foregroundStyle(row.turn?.cacheIsLow == true ? Color.orange : Color.primary)
            }
            .width(min: 70, ideal: 92)
            TableColumn(AppLocalization.string("console.usage.output")) { row in
                Text(row.turn.map { $0.outputTokens.map(formatTokenCount) ?? "—" } ?? formatTokenCount(row.stats.output))
                    .monospacedDigit()
            }
            .width(min: 48, ideal: 60)
            TableColumn(AppLocalization.string("console.usage.reasoning")) { row in
                reasoningCell(row)
            }
            .width(min: 56, ideal: 72)
            TableColumn("TTFT") { row in
                Text(row.turn?.ttftMs.map(durationText) ?? "—").monospacedDigit()
            }
            .width(min: 52, ideal: 64)
            TableColumn(AppLocalization.string("console.usage.speed")) { row in
                Text(row.turn?.outputTPS.map { String(format: "%.1f t/s", $0) } ?? "—").monospacedDigit()
            }
            .width(min: 56, ideal: 70)
        }
        .tableStyle(.inset(alternatesRowBackgrounds: true))
        .environment(\.defaultMinListRowHeight, 22)
        .font(.callout)
    }

    @ViewBuilder
    private func statusCell(_ row: UsageTableRow) -> some View {
        if let turn = row.turn {
            Image(systemName: turn.isFailed ? "xmark.circle.fill" : "checkmark.circle.fill")
                .foregroundStyle(turn.isFailed ? usageFailedColor : Color.green.opacity(0.75))
                .help(turn.isFailed ? (turn.error ?? failedSeries) : succeededSeries)
                .accessibilityLabel(turn.isFailed ? "\(failedSeries): \(turn.error ?? "")" : succeededSeries)
        } else if row.stats.errors > 0 {
            Text("\(row.stats.errors)✕")
                .font(.caption.weight(.semibold))
                .foregroundStyle(usageFailedColor)
                .help(AppLocalization.string("console.usage.failedDetail", "\(row.stats.errors)"))
        } else {
            Image(systemName: "checkmark.circle")
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func sessionCell(_ row: UsageTableRow) -> some View {
        let title = row.sessionTitle ?? row.session
        if row.isNested {
            Color.clear
        } else {
            HStack(spacing: 4) {
                if let children = row.children {
                    let isExpanded = expandedSessions.contains(row.session)
                    Button {
                        if isExpanded {
                            expandedSessions.remove(row.session)
                        } else {
                            expandedSessions.insert(row.session)
                        }
                    } label: {
                        Image(systemName: "chevron.right")
                            .font(.caption2.weight(.semibold))
                            .rotationEffect(.degrees(isExpanded ? 90 : 0))
                            .frame(width: 12)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(AppLocalization.string(
                        isExpanded ? "console.usage.collapseSession" : "console.usage.expandSession", title))
                    Button {
                        model.sessionFilter = model.sessionFilter == nil ? row.session : nil
                    } label: {
                        Text(title).lineLimit(1).truncationMode(.tail)
                    }
                    .buttonStyle(.link)
                    .help(AppLocalization.string("console.usage.sessionHelp"))
                    Text("\(children.count)")
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)
                } else {
                    Button {
                        model.sessionFilter = model.sessionFilter == nil ? row.session : nil
                    } label: {
                        Text(title).lineLimit(1).truncationMode(.tail)
                    }
                    .buttonStyle(.link)
                    .help(AppLocalization.string("console.usage.sessionHelp"))
                }
            }
            .font(row.sessionTitle == nil ? .callout.monospaced() : .callout)
        }
    }

    @ViewBuilder
    private func modelCell(_ row: UsageTableRow) -> some View {
        if let turn = row.turn {
            HStack(spacing: 6) {
                providerLogo(turn.providerID)
                Text(turn.upstreamModelID).lineLimit(1).truncationMode(.middle)
                if turn.isSubstituted, let returned = turn.returnedModel {
                    Text("→ \(returned)")
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .lineLimit(1)
                        .help(AppLocalization.string("console.usage.returned", returned))
                }
            }
            .help("\(model.providerNames[turn.providerID] ?? turn.providerID) · \(turn.upstreamModelID)")
        } else if row.models.count == 1, let only = row.models.first {
            Text(only).lineLimit(1).foregroundStyle(.secondary)
        } else {
            Text(AppLocalization.string("console.usage.multipleModels", row.models.count))
                .foregroundStyle(.secondary)
        }
    }

    private func providerLogo(_ providerID: String) -> some View {
        Group {
            if let logo = providerLogoImage(providerID, websiteURL: model.providerWebsites[providerID]) {
                Image(nsImage: logo)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
                    .clipShape(RoundedRectangle(cornerRadius: 3, style: .continuous))
            } else {
                Image(systemName: "cpu").foregroundStyle(.secondary)
            }
        }
        .frame(width: 14, height: 14)
    }

    /// Reasoning is shown in tokens like every other column. Upstreams that
    /// stream thinking without a counter show a dash and the character count
    /// in the tooltip, so the column never mixes units.
    @ViewBuilder
    private func reasoningCell(_ row: UsageTableRow) -> some View {
        if let turn = row.turn {
            if let tokens = turn.reasoningTokens {
                Text(formatTokenCount(tokens)).monospacedDigit()
            } else if turn.reasoningChars > 0 {
                Text("—")
                    .foregroundStyle(.secondary)
                    .help(AppLocalization.string("console.usage.reasoningCharsOnly", formatTokenCount(turn.reasoningChars)))
            } else {
                Text("—").foregroundStyle(.secondary)
            }
        } else {
            Text("")
        }
    }

    private func timeText(_ date: Date) -> String {
        model.range == .day
            ? date.formatted(date: .omitted, time: .standard)
            : date.formatted(.dateTime.month(.defaultDigits).day().hour().minute())
    }

    private func durationText(_ milliseconds: Double) -> String {
        milliseconds < 1_000
            ? String(format: "%.0f ms", milliseconds)
            : String(format: "%.2f s", milliseconds / 1_000)
    }

    private func cachedText(_ row: UsageTableRow) -> String {
        guard let turn = row.turn else {
            let tokens = formatTokenCount(row.stats.cached)
            return row.stats.cacheHitPercent.map { "\(tokens) · \(String(format: "%.0f%%", $0))" } ?? tokens
        }
        let tokens = turn.cacheReadTokens.map(formatTokenCount) ?? "—"
        guard let percent = turn.cacheHitPercent else { return tokens }
        return "\(tokens) · \(String(format: "%.0f%%", percent))"
    }
}

/// Whether a rise in the compared value is good, bad, or just volume.
private enum UsageDeltaTone {
    case neutral
    case higherIsBetter
    case lowerIsBetter
}

/// A headline number with its change against the previous period and a
/// trend line over the range's slots.
private struct UsageKPICard<Accessory: View>: View {
    let title: String
    let value: String
    let delta: (text: String, sign: Int)?
    let deltaTone: UsageDeltaTone
    let symbol: String
    let tint: Color
    let trend: [Double]
    var warning = false
    @ViewBuilder let accessory: () -> Accessory

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Label {
                    Text(title)
                } icon: {
                    Image(systemName: symbol).foregroundStyle(tint)
                }
                .font(.callout)
                .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                if let delta {
                    Text(AppLocalization.string("console.usage.vsPrevious", delta.text))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(deltaColor(delta.sign))
                        .lineLimit(1)
                }
            }
            HStack(alignment: .lastTextBaseline, spacing: 8) {
                Text(value)
                    .font(.system(size: 22, weight: .semibold, design: .rounded).monospacedDigit())
                    .foregroundStyle(warning ? Color.orange : Color.primary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                Spacer(minLength: 4)
                sparkline
                    .frame(width: 70, height: 22)
            }
            accessory()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(ConsoleCardBackground())
        .accessibilityElement(children: .combine)
    }

    private func deltaColor(_ sign: Int) -> Color {
        switch (deltaTone, sign) {
        case (.higherIsBetter, 1), (.lowerIsBetter, -1): return .green
        case (.higherIsBetter, -1), (.lowerIsBetter, 1): return .red
        default: return .secondary
        }
    }

    @ViewBuilder
    private var sparkline: some View {
        if trend.count > 1, trend.contains(where: { $0 > 0 }) {
            Chart(Array(trend.enumerated()), id: \.offset) { point in
                AreaMark(x: .value("Slot", point.offset), y: .value("Value", point.element))
                    .foregroundStyle(tint.opacity(0.15))
                    .interpolationMethod(.monotone)
                LineMark(x: .value("Slot", point.offset), y: .value("Value", point.element))
                    .foregroundStyle(tint)
                    .lineStyle(StrokeStyle(lineWidth: 1.3))
                    .interpolationMethod(.monotone)
            }
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartLegend(.hidden)
            .accessibilityHidden(true)
        } else {
            Color.clear
        }
    }
}

/// Card surface shared by the console's usage and context pages.
struct ConsoleCardBackground: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 10, style: .continuous)
            .fill(Color(nsColor: .controlBackgroundColor))
            .overlay(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(Color(nsColor: .separatorColor).opacity(0.6))
            )
    }
}

/// One headline number with its label and an optional detail line.
struct ConsoleMetricCard: View {
    let title: String
    let value: String
    var detail: String?
    let symbol: String
    let tint: Color
    var warning = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label {
                Text(title)
            } icon: {
                Image(systemName: symbol).foregroundStyle(tint)
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 26, weight: .semibold, design: .rounded).monospacedDigit())
                .foregroundStyle(warning ? Color.orange : Color.primary)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            if let detail {
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(ConsoleCardBackground())
        .accessibilityElement(children: .combine)
    }
}
