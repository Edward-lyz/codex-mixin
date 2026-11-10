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

@MainActor
final class ConsoleUsageModel: ObservableObject {
    typealias Loader = (_ days: Int, _ turnLimit: Int) async throws -> (
        [DailyUsageRecord], [TurnUsageRecord], [String: String], [String: String]
    )
    /// Replies inside one chart slot, read straight from the gateway ledger.
    typealias SlotLoader = (_ interval: DateInterval) async throws -> [TurnUsageRecord]

    @Published var range = ConsoleUsageRange.week
    @Published var split = false
    @Published var providerFilter: String?
    @Published var sessionFilter: String?
    /// A chart slot the user clicked; the reply table lists only its replies.
    @Published var slotFilter: UsageChartPoint? {
        didSet {
            guard slotFilter?.interval != oldValue?.interval else { return }
            loadSlotTurns()
        }
    }
    /// Replies of the selected slot; nil while none is selected or loading.
    @Published private(set) var slotTurns: [TurnUsageRecord]?
    @Published private(set) var isLoadingSlot = false
    @Published private(set) var daily: [DailyUsageRecord] = []
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

    /// Replies for the chart: provider filter only, so a clicked slot or
    /// session never empties the other bars.
    var chartTurns: [TurnUsageRecord] {
        turns.filter { providerFilter == nil || $0.providerID == providerFilter }
    }

    var filteredTurns: [TurnUsageRecord] {
        let source = slotFilter == nil ? chartTurns : (slotTurns ?? [])
        return source.filter { turn in
            (providerFilter == nil || turn.providerID == providerFilter)
                && (sessionFilter == nil || turn.session == sessionFilter)
                && (slotFilter.map { $0.interval.start <= turn.date && turn.date < $0.interval.end } ?? true)
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
            let result = try await loader(range.rawValue, range.turnLimit)
            guard generation == loadGeneration else { return }
            (daily, turns, providerNames, providerWebsites) = result
            turnsTruncated = turns.count >= range.turnLimit
            if slotFilter != nil { loadSlotTurns() }
            if let providerFilter, !providers.contains(providerFilter) {
                self.providerFilter = nil
                sessionFilter = nil
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

struct ConsoleUsageView: View {
    @ObservedObject var model: ConsoleUsageModel
    /// Overview page: headline metrics and the compact chart only, without
    /// the filter toolbar and the full reply table.
    var overview = false
    @State private var pageIndex = 0
    @State private var chartShowsRequests = false
    @State private var hoveredSlot: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if overview {
                Text(AppLocalization.string("console.overview.title"))
                    .font(.title2.weight(.semibold))
            } else {
                toolbar
            }
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
            if !overview {
                turnsCard
            }
        }
        .padding(20)
        .frame(minWidth: 860, minHeight: 620, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
        .onChange(of: model.range) { _ in pageIndex = 0 }
        .onChange(of: model.providerFilter) { _ in pageIndex = 0 }
        .onChange(of: model.sessionFilter) { _ in pageIndex = 0 }
        .onChange(of: model.slotFilter) { _ in pageIndex = 0 }
        .onChange(of: model.filteredTurns.count) { count in
            pageIndex = min(pageIndex, max(0, (count - 1) / usagePageSize))
        }
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
            if let session = model.sessionFilter {
                filterChip(model.turns.first { $0.session == session }?.sessionDisplay ?? session) {
                    model.sessionFilter = nil
                }
            }
            if let slot = model.slotFilter {
                filterChip(slotTitle(slot.start)) { model.slotFilter = nil }
            }
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
            .font(.callout)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .frame(maxWidth: 240)
            .background(Color.accentColor.opacity(0.14), in: Capsule())
        }
        .buttonStyle(.plain)
        .help(AppLocalization.string("console.usage.clearFilter"))
        .accessibilityLabel("\(AppLocalization.string("console.usage.clearFilter")): \(title)")
    }

    // MARK: Summary

    private var summary: some View {
        let totals = UsageTotals(model.filteredDaily)
        let replies = model.chartTurns
        let mismatches = replies.filter { $0.modelMatches == false }.count
        return HStack(spacing: 12) {
            metric(
                AppLocalization.string("console.usage.requests"),
                formatTokenCount(totals.requests),
                detail: totals.requests > 0
                    ? AppLocalization.string("console.usage.perRequest", formatTokenCount(totals.tokens / totals.requests))
                    : nil,
                symbol: "arrow.up.arrow.down",
                tint: .blue
            )
            metric(
                "Tokens",
                formatTokenCount(totals.tokens),
                detail: AppLocalization.string("console.usage.outputDetail", formatTokenCount(totals.output)),
                symbol: "square.stack.3d.up",
                tint: .purple
            )
            metric(
                AppLocalization.string("console.usage.cacheHit"),
                totals.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "—",
                detail: totals.cacheHitPercent == nil
                    ? nil
                    : AppLocalization.string("console.usage.cachedDetail", formatTokenCount(totals.cached)),
                symbol: "bolt",
                tint: .green
            )
            metric(
                AppLocalization.string("console.usage.mismatch"),
                "\(mismatches)",
                detail: AppLocalization.string("console.usage.repliesDetail", replies.count),
                symbol: "exclamationmark.triangle",
                tint: .secondary
            )
        }
    }

    private func metric(
        _ title: String,
        _ value: String,
        detail: String?,
        symbol: String,
        tint: Color,
        warning: Bool = false
    ) -> some View {
        ConsoleMetricCard(
            title: title, value: value, detail: detail ?? " ",
            symbol: symbol, tint: tint, warning: warning
        )
    }

    private var cardBackground: some View {
        ConsoleCardBackground()
    }

    // MARK: Chart

    private var chartCard: some View {
        let bucket = model.range.chartBucket
        let points = usageChartPoints(
            daily: model.filteredDaily,
            turns: model.chartTurns,
            bucket: bucket,
            split: model.split,
            aggregateLabel: AppLocalization.string("console.usage.allProviders"),
            lastDay: currentUnixDay(),
            days: model.range.rawValue
        )
        let value: (UsageChartPoint) -> UInt64 = chartShowsRequests ? { $0.requests } : { $0.tokens }
        let names = Array(Set(points.map { seriesName($0.series) })).sorted()
        let colors = model.split
            ? names.indices.map { usageSeriesColors[$0 % usageSeriesColors.count] }
            : [Color.accentColor]
        let starts = Array(Set(points.map(\.start))).sorted()
        let highlighted = hoveredSlot ?? model.slotFilter?.start
        let isEmpty = points.allSatisfy { value($0) == 0 }
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Picker(AppLocalization.string("console.usage.metric"), selection: $chartShowsRequests) {
                    Text("Tokens").tag(false)
                    Text(AppLocalization.string("console.usage.requests")).tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                if bucket == .hour, model.turnsTruncated {
                    Text(AppLocalization.string("console.usage.truncated", model.range.turnLimit))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Picker(AppLocalization.string("console.usage.grouping"), selection: $model.split) {
                    Text(AppLocalization.string("console.usage.merge")).tag(false)
                    Text(AppLocalization.string("console.usage.split")).tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
            Chart(points) { point in
                BarMark(
                    x: .value("Time", point.start, unit: bucket.component),
                    y: .value("Value", value(point))
                )
                .foregroundStyle(by: .value("Provider", seriesName(point.series)))
                .opacity(highlighted == nil || highlighted == point.start ? 1 : 0.35)
            }
            .chartForegroundStyleScale(domain: names, range: colors)
            .chartXScale(domain: chartDomain(starts, bucket: bucket))
            .chartXAxis { xAxis(bucket: bucket) }
            .chartYAxis {
                AxisMarks(position: .leading) {
                    AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5, dash: [3, 3]))
                    AxisValueLabel(format: FloatingPointFormatStyle<Double>(locale: Locale(identifier: "en_US"))
                        .notation(.compactName).precision(.fractionLength(0...1)))
                }
            }
            .chartLegend(model.split ? .visible : .hidden)
            .chartOverlay { proxy in
                chartInteraction(proxy: proxy, points: points, starts: starts, bucket: bucket, value: value)
            }
            .overlay {
                if isEmpty, !model.isLoading {
                    Text(AppLocalization.string("console.usage.noData"))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(14)
        .frame(height: 130)
        .background(cardBackground)
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

    /// Hover shows the slot's totals beside the bar; a click lists only that
    /// slot's replies in the table, and a second click clears it.
    private func chartInteraction(
        proxy: ChartProxy,
        points: [UsageChartPoint],
        starts: [Date],
        bucket: UsageChartBucket,
        value: @escaping (UsageChartPoint) -> UInt64
    ) -> some View {
        GeometryReader { geometry in
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
                        guard let hoveredSlot,
                              let slot = points.first(where: { $0.start == hoveredSlot })
                        else { return }
                        model.slotFilter = model.slotFilter?.start == hoveredSlot ? nil : slot
                    }
                if let hoveredSlot,
                   let center = Calendar.current.date(byAdding: bucket.component, value: 1, to: hoveredSlot)
                    .map({ hoveredSlot.addingTimeInterval($0.timeIntervalSince(hoveredSlot) / 2) }),
                   let x = proxy.position(forX: center).map({ $0 + plot.minX }) {
                    let leftHalf = x < geometry.size.width / 2
                    HStack(spacing: 0) {
                        if !leftHalf { Spacer(minLength: 0) }
                        tooltip(slot: hoveredSlot, points: points.filter { $0.start == hoveredSlot }, value: value)
                            .padding(leftHalf ? .leading : .trailing, leftHalf ? x + 12 : geometry.size.width - x + 12)
                        if leftHalf { Spacer(minLength: 0) }
                    }
                    .frame(width: geometry.size.width)
                    .padding(.top, plot.minY + 4)
                    .allowsHitTesting(false)
                }
            }
        }
    }

    private func tooltip(
        slot: Date,
        points: [UsageChartPoint],
        value: @escaping (UsageChartPoint) -> UInt64
    ) -> some View {
        let tokens = points.reduce(UInt64(0)) { $0 + $1.tokens }
        let requests = points.reduce(UInt64(0)) { $0 + $1.requests }
        let names = Array(Set(points.map { seriesName($0.series) })).sorted()
        let rows = points.filter { value($0) > 0 }.sorted { value($0) > value($1) }
        return VStack(alignment: .leading, spacing: 6) {
            Text(slotTitle(slot)).font(.callout.weight(.semibold))
            HStack(spacing: 16) {
                tooltipStat("Tokens", formatTokenCount(tokens))
                tooltipStat(AppLocalization.string("console.usage.requests"), formatTokenCount(requests))
            }
            if model.split, !rows.isEmpty {
                Divider()
                ForEach(rows) { point in
                    let name = seriesName(point.series)
                    HStack(spacing: 6) {
                        Circle()
                            .fill(usageSeriesColors[(names.firstIndex(of: name) ?? 0) % usageSeriesColors.count])
                            .frame(width: 7, height: 7)
                        Text(name).lineLimit(1)
                        Spacer(minLength: 12)
                        Text(formatTokenCount(value(point))).monospacedDigit()
                    }
                    .font(.caption)
                }
            }
            Text(AppLocalization.string("console.usage.slotHelp"))
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .frame(minWidth: 190, alignment: .leading)
        .fixedSize()
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(Color(nsColor: .separatorColor).opacity(0.6))
        )
        .shadow(color: .black.opacity(0.12), radius: 8, y: 2)
    }

    private func tooltipStat(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(title).font(.caption2).foregroundStyle(.secondary)
            Text(value).font(.system(.body, design: .rounded).weight(.semibold).monospacedDigit())
        }
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

    private var turnsCard: some View {
        let rows = model.filteredTurns
        let pageCount = max(1, (rows.count + usagePageSize - 1) / usagePageSize)
        return VStack(spacing: 0) {
            turnsTable(Array(rows.dropFirst(pageIndex * usagePageSize).prefix(usagePageSize)))
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
                Text(AppLocalization.string("console.usage.replyCount", rows.count))
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    pageIndex -= 1
                } label: {
                    Image(systemName: "chevron.left")
                }
                .disabled(pageIndex == 0)
                .help(AppLocalization.string("console.usage.previousPage"))
                .accessibilityLabel(AppLocalization.string("console.usage.previousPage"))
                Text("\(pageIndex + 1) / \(pageCount)").monospacedDigit()
                Button {
                    pageIndex += 1
                } label: {
                    Image(systemName: "chevron.right")
                }
                .disabled(pageIndex + 1 >= pageCount)
                .help(AppLocalization.string("console.usage.nextPage"))
                .accessibilityLabel(AppLocalization.string("console.usage.nextPage"))
            }
            .buttonStyle(.borderless)
            .font(.callout)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .background(cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private func turnsTable(_ rows: [TurnUsageRecord]) -> some View {
        Table(rows) {
            TableColumn(AppLocalization.string("console.usage.time")) { turn in
                Text(timeText(turn.date)).monospacedDigit()
            }
            .width(min: 70, ideal: model.range == .day ? 80 : 120)
            TableColumn(AppLocalization.string("console.usage.session")) { turn in
                Button {
                    model.sessionFilter = model.sessionFilter == nil ? turn.session : nil
                } label: {
                    Text(turn.sessionDisplay ?? turn.session)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                .buttonStyle(.link)
                .font(turn.sessionDisplay == nil ? .callout.monospaced() : .callout)
                .help(AppLocalization.string("console.usage.sessionHelp"))
            }
            .width(min: 120, ideal: 190)
            TableColumn(AppLocalization.string("console.usage.model")) { turn in
                HStack(spacing: 7) {
                    if let logo = providerLogoImage(turn.providerID, websiteURL: model.providerWebsites[turn.providerID]) {
                        Image(nsImage: logo)
                            .resizable()
                            .interpolation(.high)
                            .scaledToFit()
                            .frame(width: 18, height: 18)
                            .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
                    } else {
                        Image(systemName: "cpu")
                            .foregroundStyle(.secondary)
                            .frame(width: 18, height: 18)
                    }
                VStack(alignment: .leading, spacing: 1) {
                    Text(turn.upstreamModelID).lineLimit(1)
                    HStack(spacing: 4) {
                        Text(model.providerNames[turn.providerID] ?? turn.providerID).foregroundStyle(.secondary)
                        if turn.modelMatches == false, let returned = turn.returnedModel {
                            Text(AppLocalization.string("console.usage.returned", returned))
                                .foregroundStyle(.secondary)
                        }
                    }
                    .font(.caption)
                    .lineLimit(1)
                }
                }
            }
            .width(min: 180, ideal: 240)
            TableColumn(AppLocalization.string("console.usage.uncached")) { turn in
                Text(turn.inputTokens.map(formatTokenCount) ?? "—").monospacedDigit()
            }
            .width(min: 60, ideal: 80)
            TableColumn(AppLocalization.string("console.usage.cached")) { turn in
                Text(cachedText(turn))
                    .monospacedDigit()
                    .foregroundStyle(turn.cacheIsLow ? Color.orange : Color.primary)
            }
            .width(min: 80, ideal: 100)
            TableColumn(AppLocalization.string("console.usage.output")) { turn in
                Text(turn.outputTokens.map(formatTokenCount) ?? "—").monospacedDigit()
            }
            .width(min: 50, ideal: 64)
            TableColumn(AppLocalization.string("console.usage.reasoning")) { turn in
                Text(turn.reasoningText).monospacedDigit()
            }
            .width(min: 50, ideal: 70)
            TableColumn("TTFT") { turn in
                Text(turn.ttftMs.map(durationText) ?? "—").monospacedDigit()
            }
            .width(min: 56, ideal: 70)
            TableColumn(AppLocalization.string("console.usage.speed")) { turn in
                Text(turn.outputTPS.map { String(format: "%.1f t/s", $0) } ?? "—").monospacedDigit()
            }
            .width(min: 60, ideal: 76)
        }
        .tableStyle(.inset(alternatesRowBackgrounds: true))
    }

    private func timeText(_ date: Date) -> String {
        model.range == .day
            ? date.formatted(date: .omitted, time: .standard)
            : date.formatted(.dateTime.month(.defaultDigits).day().hour().minute().second())
    }

    private func durationText(_ milliseconds: Double) -> String {
        milliseconds < 1_000
            ? String(format: "%.0f ms", milliseconds)
            : String(format: "%.2f s", milliseconds / 1_000)
    }

    private func cachedText(_ turn: TurnUsageRecord) -> String {
        let tokens = turn.cacheReadTokens.map(formatTokenCount) ?? "—"
        guard let percent = turn.cacheHitPercent else { return tokens }
        return "\(tokens) · \(String(format: "%.0f%%", percent))"
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
