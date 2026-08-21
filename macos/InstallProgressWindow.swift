import Cocoa
import SwiftUI

@MainActor
final class InstallProgressModel: ObservableObject {
    @Published var phase = "准备中..."
    @Published var detail: String
    @Published var phases: [String] = []
    @Published var currentPhaseIndex: Int?
    @Published var determinateProgress: Double?
    @Published var state: State = .running

    let startedAt = Date()
    private(set) var finishedAt: Date?
    private var streamedPhaseCount = 0

    enum State { case running, succeeded, failed }

    init(detail: String) {
        self.detail = detail
    }

    func update(phase rawPhase: String) {
        let display = localizedProgressLabel(rawPhase)
        phase = display
        if let index = phases.firstIndex(of: display) ?? phases.firstIndex(of: rawPhase) {
            currentPhaseIndex = index
        }
    }

    func advanceStreamedPhase(_ rawPhase: String) {
        let display = localizedProgressLabel(rawPhase)
        phase = display
        guard !phases.isEmpty else { return }
        let nextIndex = min(streamedPhaseCount, phases.count - 1)
        streamedPhaseCount = min(streamedPhaseCount + 1, phases.count)
        currentPhaseIndex = nextIndex
        phases[nextIndex] = display
    }

    func advance(to index: Int) {
        guard !phases.isEmpty else { return }
        let clamped = min(max(index, 0), phases.count - 1)
        currentPhaseIndex = clamped
        phase = phases[clamped]
    }

    func setPhases(_ newPhases: [String]) {
        phases = newPhases
        streamedPhaseCount = 0
        currentPhaseIndex = nil
    }

    func finish(title: String) {
        guard state == .running else { return }
        state = .succeeded
        finishedAt = Date()
        phase = title
        determinateProgress = 1
        if !phases.isEmpty {
            currentPhaseIndex = phases.count - 1
        }
    }

    func fail(title: String, message: String) {
        guard state == .running else { return }
        state = .failed
        finishedAt = Date()
        phase = title
        detail = message
    }

    func elapsed(at date: Date) -> String {
        String(format: "已用时 %.1fs", (finishedAt ?? date).timeIntervalSince(startedAt))
    }
}

private struct InstallProgressView: View {
    @ObservedObject var model: InstallProgressModel

    var body: some View {
        HStack(alignment: .top, spacing: 24) {
            if !model.phases.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(model.phases.enumerated()), id: \.offset) { index, phase in
                        HStack(spacing: 8) {
                            Image(systemName: stepSymbol(at: index))
                                .foregroundStyle(stepColor(at: index))
                            Text(phase)
                                .font(.caption.weight(index == model.currentPhaseIndex ? .semibold : .regular))
                                .foregroundStyle(index <= (model.currentPhaseIndex ?? -1)
                                    ? Color.primary
                                    : Color.secondary)
                                .lineLimit(1)
                        }
                    }
                }
                .frame(width: 190, alignment: .leading)
            }

            VStack(alignment: .leading, spacing: 12) {
                Label(model.phase, systemImage: stateSymbol)
                    .font(.headline)
                    .foregroundStyle(stateColor)
                if model.state != .failed {
                    if let progress = model.determinateProgress {
                        ProgressView(value: progress)
                    } else {
                        ProgressView()
                            .progressViewStyle(.linear)
                    }
                }
                if !model.detail.isEmpty {
                    Text(model.detail)
                        .font(.caption)
                        .foregroundStyle(model.state == .failed ? Color.red : Color.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                TimelineView(.periodic(from: .now, by: 0.5)) { context in
                    Text(model.elapsed(at: context.date))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.tertiary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(28)
        .frame(minWidth: 500, minHeight: 170)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var stateSymbol: String {
        switch model.state {
        case .running: return "arrow.trianglehead.2.clockwise.rotate.90"
        case .succeeded: return "checkmark.circle.fill"
        case .failed: return "xmark.octagon.fill"
        }
    }

    private var stateColor: Color {
        switch model.state {
        case .running: return .primary
        case .succeeded: return .green
        case .failed: return .red
        }
    }

    private func stepSymbol(at index: Int) -> String {
        guard let current = model.currentPhaseIndex else { return "circle" }
        if index < current { return "checkmark.circle.fill" }
        if index == current { return "circle.inset.filled" }
        return "circle"
    }

    private func stepColor(at index: Int) -> Color {
        guard let current = model.currentPhaseIndex else { return .secondary }
        if index < current { return .green }
        if index == current { return .accentColor }
        return .secondary
    }
}

final class InstallProgressWindowController: NSWindowController, NSWindowDelegate {
    fileprivate static var retainedControllers: [InstallProgressWindowController] = []

    private let model: InstallProgressModel
    private let successTitle: String
    private let failureTitle: String

    init(
        title: String = "正在安装到 Codex",
        detail: String = "",
        successTitle: String = "✓ 完成",
        failureTitle: String = "✗ 失败"
    ) {
        model = InstallProgressModel(detail: detail)
        self.successTitle = successTitle
        self.failureTitle = failureTitle
        let window = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 500, height: 170),
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.title = title
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
        window.contentViewController = NSHostingController(rootView: InstallProgressView(model: model))
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func update(phase: String) { model.update(phase: phase) }
    func advanceStreamedPhase(_ rawPhase: String) { model.advanceStreamedPhase(rawPhase) }
    func advance(to index: Int) { model.advance(to: index) }

    func setDeterminateProgress(fraction: Double) {
        model.determinateProgress = min(max(fraction, 0), 1)
    }

    func setPhases(_ phases: [String]) {
        model.setPhases(phases)
        let stepHeight = max(CGFloat(phases.count) * 24, 80)
        window?.setContentSize(NSSize(width: 560, height: max(220, stepHeight + 100)))
    }

    func finish() {
        guard model.state == .running else { return }
        model.finish(title: successTitle)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            self.close()
        }
    }

    func finishAndWait() async {
        finish()
        try? await Task.sleep(nanoseconds: 850_000_000)
    }

    func fail(message: String) {
        guard model.state == .running else { return }
        model.fail(title: failureTitle, message: message)
        window?.styleMask.insert(.closable)
        window?.delegate = self
        if !InstallProgressWindowController.retainedControllers.contains(where: { $0 === self }) {
            InstallProgressWindowController.retainedControllers.append(self)
        }
    }

    func windowWillClose(_ notification: Notification) {
        InstallProgressWindowController.retainedControllers.removeAll { $0 === self }
    }
}

/// Maps CLI `MIXIN_PROGRESS` bodies (Chinese or English) to Chinese UI labels.
func localizedProgressLabel(_ raw: String) -> String {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    let mapping: [String: String] = [
        "检查本地配置与网关状态": "检查本地配置与网关状态",
        "Checking local config and gateway status": "检查本地配置与网关状态",
        "获取 Codex 配置模板": "获取 Codex 配置模板",
        "Fetching Codex config template": "获取 Codex 配置模板",
        "获取可用模型列表": "获取可用模型列表",
        "Fetching available models": "获取可用模型列表",
        "Refreshing model list for provider": "刷新 provider 模型列表",
        "Discovered": "已发现模型",
        "Probing capabilities for": "探测模型能力",
        "Model refresh complete for": "模型刷新完成",
        "Model refresh failed for": "模型刷新失败",
        "Capability probing failed for": "模型能力探测失败",
        "加载模型元数据": "加载模型元数据",
        "Loading model metadata": "加载模型元数据",
        "准备或安装 Codex CLI": "准备或安装 Codex CLI",
        "Preparing or installing Codex CLI": "准备或安装 Codex CLI",
        "写入 Codex 配置和模型目录": "写入 Codex 配置和模型目录",
        "Writing Codex config and model catalog": "写入 Codex 配置和模型目录",
        "同步历史会话与 SQLite 状态": "同步历史会话与 SQLite 状态",
        "Syncing history sessions and SQLite state": "同步历史会话与 SQLite 状态",
        "校验安装结果": "校验安装结果",
        "Validating install result": "校验安装结果",
        "读取并锁定 Codex 配置": "读取并锁定 Codex 配置",
        "Reading and locking Codex config": "读取并锁定 Codex 配置",
        "恢复安装前配置与登录状态": "恢复安装前配置与登录状态",
        "Restoring pre-install config and login state": "恢复安装前配置与登录状态",
        "恢复历史会话与 SQLite 状态": "恢复历史会话与 SQLite 状态",
        "Restoring history sessions and SQLite state": "恢复历史会话与 SQLite 状态",
    ]
    if let exact = mapping[trimmed] {
        return exact
    }
    for (prefix, localized) in mapping where trimmed.hasPrefix(prefix) {
        return localized + trimmed.dropFirst(prefix.count)
    }
    return trimmed
}

final class OperationProgress {
    let window: InstallProgressWindowController

    @MainActor
    init(
        title: String,
        phases: [String] = [],
        detail: String = "",
        successTitle: String = "✓ 完成",
        failureTitle: String = "✗ 失败"
    ) {
        window = InstallProgressWindowController(
            title: title,
            detail: detail,
            successTitle: successTitle,
            failureTitle: failureTitle
        )
        if !phases.isEmpty {
            window.setPhases(phases)
            window.advance(to: 0)
        }
        window.present()
    }

    func update(phase: String) {
        onMain { self.window.update(phase: phase) }
    }

    func advance(to index: Int) {
        onMain { self.window.advance(to: index) }
    }

    func advanceStreamedPhase(_ rawPhase: String) {
        onMain { self.window.advanceStreamedPhase(rawPhase) }
    }

    func setDeterminateProgress(fraction: Double) {
        onMain { self.window.setDeterminateProgress(fraction: fraction) }
    }

    func finish() {
        onMain { self.window.finish() }
    }

    func finishAndWait() async {
        await MainActor.run {
            self.window.finish()
        }
        try? await Task.sleep(nanoseconds: 850_000_000)
    }

    func fail(message: String) {
        onMain { self.window.fail(message: message) }
    }

    private func onMain(_ body: @escaping () -> Void) {
        if Thread.isMainThread {
            body()
        } else {
            DispatchQueue.main.async(execute: body)
        }
    }
}

func runOperationProgress<T>(
    title: String,
    phases: [String] = [],
    detail: String = "",
    successTitle: String = "✓ 完成",
    failureTitle: String = "✗ 失败",
    showFailureAlert: Bool = false,
    failureAlertTitle: String? = nil,
    work: (OperationProgress) async throws -> T
) async rethrows -> T {
    let progress = await MainActor.run {
        OperationProgress(
            title: title,
            phases: phases,
            detail: detail,
            successTitle: successTitle,
            failureTitle: failureTitle
        )
    }
    do {
        let result = try await work(progress)
        // Wait until the success window has closed so callers do not race it
        // with a modal alert while the controller is already deallocated.
        await progress.finishAndWait()
        return result
    } catch {
        progress.fail(message: localizedErrorDescription(error))
        if showFailureAlert {
            showAlert(
                title: failureAlertTitle ?? title,
                message: String(describing: error)
            )
        }
        throw error
    }
}
