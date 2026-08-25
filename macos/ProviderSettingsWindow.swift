import Cocoa
import SwiftUI

final class ProviderSettingsWindowController: NSWindowController, NSWindowDelegate {
    typealias LoadHandler = () async throws -> ProviderListResponse
    typealias RunHandler = ([String]) async throws -> String
    typealias ApplyHandler = (_ progress: OperationProgress?) async throws -> Void
    typealias BaiduBridgeSetupHandler = (BaiduAuthBridgeMode) async throws -> URL
    typealias CompletionHandler = (_ title: String, _ message: String) -> Void

    private let loadHandler: LoadHandler
    private let runHandler: RunHandler
    private let applyHandler: ApplyHandler
    private let baiduBridgeSetupHandler: BaiduBridgeSetupHandler?
    private let completionHandler: CompletionHandler?

    let model = ProviderSettingsModel()
    private var selectedProvider: ProviderView? {
        model.selectedProvider
    }
    private var remindedBaiduBridgeProviderIDs = Set<String>()
    private var bannerHideWorkItem: DispatchWorkItem?

    init(
        loadHandler: @escaping LoadHandler,
        runHandler: @escaping RunHandler,
        applyHandler: @escaping ApplyHandler,
        baiduBridgeSetupHandler: BaiduBridgeSetupHandler? = nil,
        completionHandler: CompletionHandler? = nil
    ) {
        self.loadHandler = loadHandler
        self.runHandler = runHandler
        self.applyHandler = applyHandler
        self.baiduBridgeSetupHandler = baiduBridgeSetupHandler
        self.completionHandler = completionHandler
        let visibleFrame = NSScreen.main?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: 1_280, height: 800)
        let contentSize = providerSettingsContentSize(for: visibleFrame)
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: contentSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "供应商设置"
        window.minSize = NSSize(width: 820, height: 580)
        window.isReleasedWhenClosed = false
        window.toolbarStyle = .unified
        if #available(macOS 26.0, *) {
            window.titlebarAppearsTransparent = true
        }
        window.center()
        super.init(window: window)
        window.delegate = self
        installContent()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present() {
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        reloadProviders()
    }

    private func installContent() {
        let rootView = ProviderSettingsRootView(
            model: model,
            onAdd: { [weak self] in self?.addProvider() },
            onRemove: { [weak self] in self?.removeProvider() },
            onToggle: { [weak self] in self?.toggleProvider() },
            onTest: { [weak self] in self?.testProvider() },
            onSave: { [weak self] in self?.saveProvider() },
            onClearKey: { [weak self] in self?.clearProviderKey() },
            onClearQuotaCredentials: { [weak self] in self?.clearQuotaCredentials() },
            onMove: { [weak self] offsets, destination in
                self?.moveProviders(from: offsets, to: destination)
            }
        )
        window?.contentViewController = NSHostingController(rootView: rootView)
    }

    func moveProviders(from offsets: IndexSet, to destination: Int) {
        guard offsets.count == 1,
              let source = offsets.first,
              model.providers.indices.contains(source),
              model.providers[source].kind == .configured,
              !model.isBusy
        else {
            return
        }

        let firstConfigured = model.providers.firstIndex(where: { $0.kind == .configured })
            ?? model.providers.count
        var reordered = model.providers
        let movedProvider = reordered.remove(at: source)
        let requestedInsertion = source < destination ? destination - 1 : destination
        let insertion = min(max(requestedInsertion, firstConfigured), reordered.count)
        reordered.insert(movedProvider, at: insertion)
        model.providers = reordered
        model.selectProvider(movedProvider.id)
        persistProviderOrder(selecting: movedProvider.id)
    }

    private func persistProviderOrder(selecting providerID: String) {
        let ids = model.providers
            .filter { $0.kind == .configured }
            .map(\.id)
        guard !model.isBusy else { return }
        setBusy(true, status: "正在保存 Provider 顺序…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                _ = try await runHandler(["providers", "reorder"] + ids)
                setBusy(false, status: "Provider 顺序已保存，正在核对配置…")
                reloadProviders(selecting: providerID)
            } catch {
                setBusy(false, status: "Provider 顺序保存失败")
                showAlert(title: "保存 Provider 顺序失败", message: String(describing: error))
                reloadProviders(selecting: providerID)
            }
        }
    }

    private func reloadProviders(selecting providerID: String? = nil) {
        guard !model.isBusy else { return }
        setBusy(true, status: "正在读取供应商…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                setBusy(
                    false,
                    status: selectedProviderStatus(
                        provider: selectedProvider,
                        providersEmpty: model.providers.isEmpty,
                        codexInstallMode: model.codexInstallMode
                    )
                )
            }
            do {
                let previousID = providerID ?? selectedProvider?.id
                let loaded = try await loadHandler()
                model.codexInstallMode = loaded.codexInstallMode
                model.providers = loaded.providers
                if let previousID, model.providers.contains(where: { $0.id == previousID }) {
                    model.selectProvider(previousID)
                } else {
                    model.selectProvider(model.providers.first?.id)
                }
                DispatchQueue.main.async { [weak self] in
                    self?.showBaiduBridgeReminderIfNeeded()
                }
            } catch {
                model.isBusy = false
                model.status = "读取失败"
                showAlert(title: "读取供应商失败", message: String(describing: error))
            }
        }
    }

    private func loadSelectedProvider() {
        model.selectProvider(model.selectedProviderID)
    }

    private func showBanner(title: String, message: String, isError: Bool) {
        bannerHideWorkItem?.cancel()
        let text = message.isEmpty ? title : "\(title)：\(message)"
        model.banner = ProviderBannerState(text: text, isError: isError)

        let workItem = DispatchWorkItem { [weak self] in
            self?.model.banner = nil
            self?.bannerHideWorkItem = nil
        }
        bannerHideWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + 5, execute: workItem)
    }

    private func setBusy(_ busy: Bool, status: String) {
        model.isBusy = busy
        model.status = status
    }


    func addProvider() {
        guard !model.isBusy, let window else { return }
        runAddProviderSheet(attachedTo: window) { [weak self] values in
            guard let self, let values else { return }
            submitNewProvider(values)
        }
    }

    private func submitNewProvider(_ values: AddProviderFormValues) {
        let id = nextProviderID(for: values.preset)
        let key = values.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            showAlert(title: "缺少 API 密钥", message: "新增 Provider 必须填写 API 密钥。")
            return
        }
        var arguments = ["providers", "add", "--preset", values.preset, "--id", id, "--key", key]
        if values.preset == "custom" {
            appendProviderArgument(&arguments, "--display-name", values.displayName)
            appendProviderArgument(&arguments, "--base-url", values.baseURL)
            appendProviderArgument(&arguments, "--website-url", values.websiteURL)
        }
        appendProviderArgument(&arguments, "--quota-username", values.quotaUsername)
        if requiresOpenCodeGoQuotaCredentials(values.preset) {
            appendProviderArgument(
                &arguments,
                "--quota-workspace-id",
                values.quotaWorkspaceID
            )
            appendProviderArgument(
                &arguments,
                "--quota-auth-cookie",
                values.quotaAuthCookie
            )
        }
        let bridgeMode = BaiduAuthBridgeMode(rawValue: values.baiduAuthBridge) ?? .disabled
        if values.preset == "baidu-oneapi" {
            appendBaiduAuthBridgeArguments(&arguments, mode: bridgeMode)
        }
        performMutation(
            arguments,
            status: "正在新增并发现模型 \(id)…",
            selecting: id,
            requiresBaiduBridge: values.preset == "baidu-oneapi" ? bridgeMode : nil
        )
    }

    func removeProvider() {
        guard let provider = selectedProvider, provider.kind == .configured, !model.isBusy else { return }
        guard confirm(
            title: "删除 \(provider.displayName)？",
            message: "将删除 Provider \(provider.id) 的地址、密钥和模型选择。被 Fusion 引用时 CLI 会拒绝删除。"
        ) else { return }
        performMutation(
            ["providers", "remove", provider.id],
            status: "正在删除 \(provider.id)…",
            selecting: nil
        )
    }

    func toggleProvider() {
        guard let provider = selectedProvider, provider.kind == .configured, !model.isBusy else { return }
        let action = provider.enabled ? "disable" : "enable"
        performMutation(
            ["providers", action, provider.id],
            status: "正在\(provider.enabled ? "停用" : "启用") \(provider.id)…",
            selecting: provider.id
        )
    }

    func testProvider() {
        guard let provider = selectedProvider, provider.kind == .configured, !model.isBusy else { return }
        let selectedBridge = model.baiduAuthBridge
        var arguments = ["providers", "test", provider.id, "--json"]
        appendProviderArgument(&arguments, "--key", model.apiKey)
        if provider.presetID == "custom" {
            appendProviderArgument(&arguments, "--base-url", model.baseURL)
        }
        if provider.presetID == "baidu-oneapi",
           selectedBridge != (provider.effectiveBaiduAuthBridge ?? .disabled) {
            arguments.append(contentsOf: ["--baidu-auth-bridge", selectedBridge.rawValue])
        }
        setBusy(true, status: "正在测试 \(provider.id)…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                setBusy(
                    false,
                    status: selectedProviderStatus(
                        provider: selectedProvider,
                        providersEmpty: model.providers.isEmpty,
                        codexInstallMode: model.codexInstallMode
                    )
                )
            }
            do {
                if provider.presetID == "baidu-oneapi",
                   selectedBridge == .ducxLoopback,
                   selectedBridge != (provider.effectiveBaiduAuthBridge ?? .disabled) {
                    let executable = try await ensureBaiduBridgeAvailable(selectedBridge)
                    arguments.append(contentsOf: ["--ducx-executable", executable.path])
                }
                let output = try await runHandler(arguments)
                let result = try decodeProviderTest(output)
                let mode = result.mode == "configuration" ? "静态模型配置检查" : "模型接口检查"
                showAlert(
                    title: "连接测试通过",
                    message: "\(provider.displayName)：\(mode)，发现 \(result.modelCount) 个模型；没有发起付费推理。"
                )
            } catch {
                showAlert(title: "连接测试失败", message: String(describing: error))
            }
        }
    }

    func clearProviderKey() {
        guard let provider = selectedProvider,
              provider.kind == .configured,
              !model.isBusy,
              provider.apiKeyConfigured
        else { return }
        guard !provider.enabled else {
            showAlert(
                title: "请先停用 Provider",
                message: "为避免让启用中的 Provider 进入无密钥状态，请先停用 \(provider.displayName)，再清除密钥。"
            )
            return
        }
        guard confirm(
            title: "清除 \(provider.displayName) 的密钥？",
            message: "此操作会永久移除已保存的 API 密钥。之后必须重新填写密钥才能启用该 Provider。"
        ) else { return }
        performMutation(
            ["providers", "update", provider.id, "--clear-key"],
            status: "正在清除 \(provider.id) 的密钥…",
            selecting: provider.id
        )
    }

    func clearQuotaCredentials() {
        guard let provider = selectedProvider, provider.kind == .configured, !model.isBusy else { return }
        guard requiresOpenCodeGoQuotaCredentials(provider.presetID ?? ""),
              provider.quotaAuthCookieConfigured == true
        else {
            return
        }
        guard confirm(
            title: AppLocalization.string("providerSettings.clearOpenCodeGoQuotaCredentials"),
            message: AppLocalization.string("providerSettings.thisRemovesTheWorkspaceIDAndAuth")
        ) else { return }
        performMutation(
            ["providers", "update", provider.id, "--clear-quota"],
            status: "正在清除 \(provider.id) 的额度凭据…",
            selecting: provider.id
        )
    }

    private func showOpenCodeGoQuotaCredentialsAlert() {
        showAlert(
            title: AppLocalization.string("providerSettings.opencodeGoQuotaCredentialsRequired"),
            message: AppLocalization.string("providerSettings.opencodeGoRequiresBothTheWorkspaceID")
        )
    }

    func saveProvider() {
        guard let provider = selectedProvider, provider.kind == .configured, !model.isBusy else { return }
        let auxiliaryModelUpstream = model.auxiliaryModelUpstream
        var update = ["providers", "update", provider.id]
        update.append("--auxiliary-model-upstream")
        update.append(auxiliaryModelUpstream ? "true" : "false")
        appendProviderArgument(&update, "--key", model.apiKey)
        let imageGenerationPath = model.imageGenerationPath
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if imageGenerationPath.isEmpty {
            if provider.imageGenerationPath != nil {
                update.append("--clear-image-generation")
            }
        } else {
            update.append("--image-generation-path")
            update.append(imageGenerationPath)
        }
        if provider.presetID == "custom" {
            let displayName = model.displayName
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let baseURL = model.baseURL
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !displayName.isEmpty, !baseURL.isEmpty else {
                showAlert(
                    title: AppLocalization.string("providerSettings.customSiteInformationRequired"),
                    message: AppLocalization.string("providerSettings.siteNameAndAPIURLCannotBe")
                )
                return
            }
            appendProviderArgument(&update, "--display-name", displayName)
            appendProviderArgument(&update, "--base-url", baseURL)
            let websiteURL = model.websiteURL
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if !websiteURL.isEmpty || provider.websiteURL != nil {
                update.append(contentsOf: ["--website-url", websiteURL])
            }
        }
        let quotaUsername = model.quotaUsername
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if provider.presetID == "baidu-oneapi", quotaUsername.isEmpty {
            showAlert(
                title: "缺少额度用户名",
                message: "Baidu OneAPI 查询额度必须填写用户名。"
            )
            return
        }
        let selectedBaiduBridge = model.baiduAuthBridge
        if provider.presetID == "baidu-oneapi" {
            appendProviderArgument(&update, "--quota-username", quotaUsername)
            appendBaiduAuthBridgeArguments(&update, mode: selectedBaiduBridge)
            update.append(contentsOf: [
                "--baidu-code-report",
                model.baiduCodeReport ? "true" : "false",
            ])
        }
        if requiresOpenCodeGoQuotaCredentials(provider.presetID ?? "") {
            let workspaceID = model.quotaWorkspaceID
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let authCookie = model.quotaAuthCookie
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let credentialsUnchanged =
                workspaceID == (provider.quotaWorkspaceID ?? "") && authCookie.isEmpty
            if !credentialsUnchanged {
                guard !workspaceID.isEmpty, !authCookie.isEmpty else {
                    showOpenCodeGoQuotaCredentialsAlert()
                    return
                }
                appendProviderArgument(&update, "--quota-workspace-id", workspaceID)
                appendProviderArgument(&update, "--quota-auth-cookie", authCookie)
            }
        }
        performMutation(
            update,
            status: "正在保存 \(provider.id)…",
            selecting: provider.id,
            requiresBaiduBridge: provider.presetID == "baidu-oneapi"
                && baiduBridgeNeedsSetup(
                    current: provider.effectiveBaiduAuthBridge,
                    selected: selectedBaiduBridge
                ) ? selectedBaiduBridge : nil,
            codexSkillChanged: auxiliaryModelUpstream != provider.auxiliaryModelUpstream
        )
    }

    private func showBaiduBridgeReminderIfNeeded() {
        guard !model.isBusy, let window else { return }
        guard let provider = model.providers.first(where: {
            $0.presetID == "baidu-oneapi"
                && $0.effectiveBaiduAuthBridge == nil
                && !remindedBaiduBridgeProviderIDs.contains($0.id)
        }) else { return }
        remindedBaiduBridgeProviderIDs.insert(provider.id)

        let alert = NSAlert()
        alert.alertStyle = .informational
        alert.messageText = AppLocalization.string("providerSettings.chooseABaiduAuthBridge")
        alert.informativeText = AppLocalization.string("providerSettings.ducxUsesACodexMixinManagedCopy")
        alert.addButton(withTitle: AppLocalization.string("providerSettings.configureDUCX"))
        alert.addButton(withTitle: AppLocalization.string("providerSettings.keepDisabled"))
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            switch response {
            case .alertFirstButtonReturn:
                configureBaiduBridgeFromReminder(provider, mode: .ducxLoopback)
            default:
                persistBaiduBridgeDisabled(provider.id)
            }
        }
    }

    private func configureBaiduBridgeFromReminder(
        _ provider: ProviderView,
        mode: BaiduAuthBridgeMode
    ) {
        guard !model.isBusy else { return }
        let name = baiduBridgeDisplayName(mode)
        setBusy(true, status: "正在打开终端配置 \(name)…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在配置 \(name)",
                    phases: [
                        "打开终端完成登录",
                        "写入认证桥接",
                        "重启本地网关",
                        "完成",
                    ],
                    successTitle: "✓ \(name) 已配置",
                    failureTitle: "✗ 配置失败",
                    showFailureAlert: true,
                    failureAlertTitle: "供应商操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    let executable = try await self.ensureBaiduBridgeAvailable(mode)
                    progress.advance(to: 1)
                    var arguments = ["providers", "update", provider.id]
                    appendBaiduAuthBridgeArguments(
                        &arguments,
                        mode: mode,
                        executable: executable
                    )
                    _ = try await self.runHandler(arguments)
                    progress.advance(to: 2)
                    self.setBusy(true, status: "正在重启网关并应用 \(name) 配置…")
                    try await self.applyHandler(progress)
                    progress.advance(to: 3)
                    self.loadSelectedProvider()
                }
                setBusy(false, status: "\(name) 已配置，网关已重启")
                try await Task.sleep(nanoseconds: 350_000_000)
                close()
                let title = AppLocalization.string("providerSettings.configured", name)
                let message = AppLocalization.string(
                    "providerSettings.downloadAndLoginAreCompleteTheProvider",
                    name,
                    name
                )
                if let completionHandler {
                    completionHandler(title, message)
                } else if window != nil {
                    showBanner(title: title, message: message, isError: false)
                } else {
                    showAlert(title: title, message: message)
                }
            } catch {
                setBusy(false, status: "\(name) 配置失败")
                reloadProviders(selecting: provider.id)
            }
        }
    }

    private func persistBaiduBridgeDisabled(_ providerID: String) {
        guard !model.isBusy else { return }
        setBusy(true, status: "正在保持认证桥接关闭…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在更新认证桥接",
                    phases: [
                        "写入供应商配置",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    successTitle: "✓ 认证桥接已关闭",
                    failureTitle: "✗ 操作失败",
                    showFailureAlert: true,
                    failureAlertTitle: "供应商操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    var arguments = ["providers", "update", providerID]
                    appendBaiduAuthBridgeArguments(&arguments, mode: .disabled)
                    _ = try await self.runHandler(arguments)
                    try await self.applyHandler(progress)
                }
                setBusy(false, status: "认证桥接保持关闭")
                reloadProviders(selecting: providerID)
            } catch {
                setBusy(false, status: "操作失败")
            }
        }
    }

    private func nextProviderID(for preset: String) -> String {
        let existing = Set(model.providers.map(\.id))
        if !existing.contains(preset) {
            return preset
        }
        var suffix = 2
        while existing.contains("\(preset)-\(suffix)") {
            suffix += 1
        }
        return "\(preset)-\(suffix)"
    }

    private func performMutation(
        _ initialArguments: [String],
        then secondArguments: [String]? = nil,
        status: String,
        selecting providerID: String?,
        requiresBaiduBridge: BaiduAuthBridgeMode? = nil,
        codexSkillChanged: Bool = false
    ) {
        guard !model.isBusy else { return }
        setBusy(true, status: status)
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await runOperationProgress(
                    title: "正在更新供应商配置",
                    phases: [
                        "写入供应商配置",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    detail: status,
                    successTitle: "✓ 配置已保存",
                    failureTitle: "✗ 操作失败",
                    showFailureAlert: true,
                    failureAlertTitle: "供应商操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    var arguments = initialArguments
                    if let mode = requiresBaiduBridge, mode != .disabled {
                        let executable = try await self.ensureBaiduBridgeAvailable(mode)
                        appendBaiduAuthBridgeExecutable(
                            &arguments,
                            mode: mode,
                            executable: executable
                        )
                    }
                    _ = try await self.runHandler(arguments)
                    if let secondArguments {
                        _ = try await self.runHandler(secondArguments)
                    }
                    try await self.applyHandler(progress)
                }
                setBusy(false, status: "配置已保存")
                reloadProviders(selecting: providerID)
                showAlert(
                    title: AppLocalization.string("providerSettings.providerConfigurationUpdated"),
                    message: codexSkillChanged
                        ? "生图 Skill 已更新。请重启 Codex 后新建线程，使 Codex 重新加载 Skill。"
                        : AppLocalization.string("providerSettings.theCodexModelCatalogHasBeenRegenerated")
                )
            } catch {
                setBusy(false, status: "操作失败")
                reloadProviders(selecting: providerID)
            }
        }
    }

    private func ensureBaiduBridgeAvailable(_ mode: BaiduAuthBridgeMode) async throws -> URL {
        if let baiduBridgeSetupHandler {
            return try await baiduBridgeSetupHandler(mode)
        }
        switch mode {
        case .ducxLoopback:
            setBusy(true, status: "请在终端完成 DUCX 下载与扫码登录…")
            return try await setupDucxInTerminal()
        case .disabled:
            throw GatewayError.command("关闭认证桥接不需要安装客户端。")
        }
    }
}
