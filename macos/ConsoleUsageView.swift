import Charts
import SwiftUI

enum ConsoleUsageRange: Int, CaseIterable, Identifiable {
    case day = 1
    case week = 7
    case month = 30

    var id: Int { rawValue }
    var title: String { AppLocalization.string("console.usage.days", rawValue) }
}

@MainActor
final class ConsoleUsageModel: ObservableObject {
    typealias Loader = (_ days: Int) async throws -> ([DailyUsageRecord], [TurnUsageRecord])

    @Published var range = ConsoleUsageRange.week
    @Published var split = false
    @Published var providerFilter: String?
    @Published var sessionFilter: String?
    @Published private(set) var daily: [DailyUsageRecord] = []
    @Published private(set) var turns: [TurnUsageRecord] = []
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?
    private let loader: Loader

    init(loader: @escaping Loader) {
        self.loader = loader
    }

    var providers: [String] {
        Array(Set(daily.map(\.providerID) + turns.map(\.providerID))).sorted()
    }

    var filteredDaily: [DailyUsageRecord] {
        daily.filter { providerFilter == nil || $0.providerID == providerFilter }
    }

    var filteredTurns: [TurnUsageRecord] {
        turns.filter {
            (providerFilter == nil || $0.providerID == providerFilter)
                && (sessionFilter == nil || $0.session == sessionFilter)
        }
    }

    private var loadGeneration = 0

    /// Starts a load for the current range. A newer load supersedes older
    /// ones, so switching range mid-load never shows the old range's rows.
    func refresh() async {
        loadGeneration += 1
        let generation = loadGeneration
        let days = range.rawValue
        isLoading = true
        do {
            let result = try await loader(days)
            guard generation == loadGeneration else { return }
            (daily, turns) = result
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
        Task { await refresh() }
    }
}

struct ConsoleUsageView: View {
    @ObservedObject var model: ConsoleUsageModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            toolbar
            if let message = model.errorMessage {
                Text(message).foregroundStyle(.red).font(.callout)
            }
            summary
            charts
            turnsTable
        }
        .padding(20)
        .frame(minWidth: 860, minHeight: 560, alignment: .topLeading)
        .background(Color(nsColor: .windowBackgroundColor))
        .task { await model.refresh() }
    }

    private var toolbar: some View {
        HStack(spacing: 12) {
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
                ForEach(model.providers, id: \.self) { Text($0).tag(String?.some($0)) }
            }
            .fixedSize()
            Picker(AppLocalization.string("console.usage.grouping"), selection: $model.split) {
                Text(AppLocalization.string("console.usage.merge")).tag(false)
                Text(AppLocalization.string("console.usage.split")).tag(true)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            Spacer()
            if model.isLoading { ProgressView().controlSize(.small) }
            Button {
                Task { await model.refresh() }
            } label: {
                Label(AppLocalization.string("agentManagement.refresh"), systemImage: "arrow.clockwise")
            }
            .keyboardShortcut("r", modifiers: .command)
            .disabled(model.isLoading)
        }
    }

    private var summary: some View {
        let totals = UsageTotals(model.filteredDaily)
        let mismatches = model.filteredTurns.filter { $0.modelMatches == false }.count
        return HStack(spacing: 12) {
            metric(AppLocalization.string("console.usage.requests"), formatTokenCount(totals.requests))
            metric("Tokens", formatTokenCount(totals.tokens))
            metric(
                AppLocalization.string("console.usage.cacheHit"),
                totals.cacheHitPercent.map { String(format: "%.1f%%", $0) } ?? "—"
            )
            metric(AppLocalization.string("console.usage.mismatch"), "\(mismatches)", warning: mismatches > 0)
        }
    }

    private func metric(_ title: String, _ value: String, warning: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Text(value)
                .font(.title2.weight(.semibold).monospacedDigit())
                .foregroundStyle(warning ? Color.red : Color.primary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .combine)
    }

    private var charts: some View {
        let points = usageChartPoints(
            model.filteredDaily,
            split: model.split,
            aggregateLabel: AppLocalization.string("console.usage.allProviders"),
            lastDay: currentUnixDay(),
            days: model.range.rawValue
        )
        return HStack(spacing: 16) {
            chart("Tokens", points) { $0.tokens }
            chart(AppLocalization.string("console.usage.requests"), points) { $0.requests }
        }
        .frame(height: 170)
    }

    private func chart(
        _ title: String,
        _ points: [UsageChartPoint],
        value: @escaping (UsageChartPoint) -> UInt64
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Chart(points) { point in
                BarMark(
                    x: .value("Day", point.dayLabel),
                    y: .value(title, value(point))
                )
                .foregroundStyle(by: .value("Provider", point.series))
            }
            .chartLegend(model.split ? .visible : .hidden)
        }
        .frame(maxWidth: .infinity)
    }

    private var turnsTable: some View {
        Table(model.filteredTurns) {
            TableColumn(AppLocalization.string("console.usage.time")) { turn in
                Text(turn.date.formatted(date: .numeric, time: .standard)).monospacedDigit()
            }
            .width(min: 130, ideal: 150)
            TableColumn(AppLocalization.string("console.usage.session")) { turn in
                Button(turn.session) {
                    model.sessionFilter = model.sessionFilter == nil ? turn.session : nil
                }
                .buttonStyle(.link)
                .font(.caption.monospaced())
                .help(AppLocalization.string("console.usage.sessionHelp"))
            }
            .width(80)
            TableColumn(AppLocalization.string("console.usage.model")) { turn in
                VStack(alignment: .leading, spacing: 1) {
                    Text(turn.upstreamModelID)
                    HStack(spacing: 4) {
                        Text(turn.providerID).foregroundStyle(.secondary)
                        if turn.modelMatches == false, let returned = turn.returnedModel {
                            Text(AppLocalization.string("console.usage.returned", returned))
                                .foregroundStyle(.red)
                        }
                    }
                    .font(.caption)
                }
            }
            .width(min: 160, ideal: 220)
            TableColumn(AppLocalization.string("console.usage.uncached")) { turn in
                Text(turn.inputTokens.map(formatTokenCount) ?? "—").monospacedDigit()
            }
            TableColumn(AppLocalization.string("console.usage.cached")) { turn in
                Text(cachedText(turn))
                    .monospacedDigit()
                    .foregroundStyle(turn.cacheIsLow ? Color.red : Color.primary)
            }
            TableColumn(AppLocalization.string("console.usage.output")) { turn in
                Text(turn.outputTokens.map(formatTokenCount) ?? "—").monospacedDigit()
            }
            TableColumn(AppLocalization.string("console.usage.reasoning")) { turn in
                Text(turn.reasoningText).monospacedDigit()
            }
            TableColumn("TTFT") { turn in
                Text(turn.ttftMs.map { String(format: "%.0f ms", $0) } ?? "—").monospacedDigit()
            }
            TableColumn(AppLocalization.string("console.usage.speed")) { turn in
                Text(turn.outputTPS.map { String(format: "%.1f t/s", $0) } ?? "—").monospacedDigit()
            }
        }
    }

    private func cachedText(_ turn: TurnUsageRecord) -> String {
        let tokens = turn.cacheReadTokens.map(formatTokenCount) ?? "—"
        guard let percent = turn.cacheHitPercent else { return tokens }
        return "\(tokens) · \(String(format: "%.0f%%", percent))"
    }
}
