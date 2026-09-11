import Cocoa
import SwiftUI

private let providerStateRefreshIntervalNanoseconds: UInt64 = 10_000_000_000

private struct ProviderUpdatePlan {
    var arguments: [String]
    let requiresBaiduBridge: BaiduAuthBridgeMode?
    let codexSkillChanged: Bool
}

final class ProviderSettingsWindowController: NSWindowController, NSWindowDelegate {
    typealias LoadHandler = () async throws -> ProviderListResponse
    typealias RunHandler = ([String]) async throws -> String
    typealias ApplyHandler = (_ progress: OperationProgress?) async throws -> Void
    typealias BenchmarkStartHandler = (Int, String, Int) async throws -> ModelBenchmarkSnapshot
    typealias BenchmarkFetchHandler = () async throws -> ModelBenchmarkSnapshot?
    typealias SaveModelSelectionHandler = (ProviderModelSelectionUpdate) async throws -> Void
    typealias BaiduBridgeSetupHandler = (BaiduAuthBridgeMode) async throws -> URL
    typealias CompletionHandler = (_ title: String, _ message: String) -> Void

    private let loadHandler: LoadHandler
    private let runHandler: RunHandler
    private let applyHandler: ApplyHandler
    private let baiduBridgeSetupHandler: BaiduBridgeSetupHandler?
    private let completionHandler: CompletionHandler?
    private let saveModelSelectionHandler: SaveModelSelectionHandler
    private let backgroundRefreshIntervalNanoseconds: UInt64

    let model = ProviderSettingsModel()
    let benchmarkModel: ModelBenchmarkModel
    private var selectedProvider: ProviderView? {
        model.selectedProvider
    }
    private var bannerHideWorkItem: DispatchWorkItem?
    private var backgroundRefreshTask: Task<Void, Never>?

    init(
        loadHandler: @escaping LoadHandler,
        runHandler: @escaping RunHandler,
        applyHandler: @escaping ApplyHandler,
        benchmarkStartHandler: @escaping BenchmarkStartHandler = { _, _, _ in
            throw GatewayError.command("测速功能未配置")
        },
        benchmarkFetchHandler: @escaping BenchmarkFetchHandler = { nil },
        saveModelSelectionHandler: @escaping SaveModelSelectionHandler = { _ in
            throw GatewayError.command("模型选择保存功能未配置")
        },
        baiduBridgeSetupHandler: BaiduBridgeSetupHandler? = nil,
        completionHandler: CompletionHandler? = nil,
        backgroundRefreshIntervalNanoseconds: UInt64? = nil
    ) {
        self.loadHandler = loadHandler
        self.runHandler = runHandler
        self.applyHandler = applyHandler
        self.saveModelSelectionHandler = saveModelSelectionHandler
        self.baiduBridgeSetupHandler = baiduBridgeSetupHandler
        self.completionHandler = completionHandler
        self.backgroundRefreshIntervalNanoseconds = backgroundRefreshIntervalNanoseconds
            ?? providerStateRefreshIntervalNanoseconds
        benchmarkModel = ModelBenchmarkModel(
            startHandler: benchmarkStartHandler,
            fetchHandler: benchmarkFetchHandler,
            loadProvidersHandler: loadHandler,
            saveSelectionsHandler: { selections, contexts, progress in
                progress.advance(to: 0)
                for providerID in selections.keys.sorted() {
                    try await saveModelSelectionHandler(ProviderModelSelectionUpdate(
                        providerID: providerID,
                        modelIDs: selections[providerID] ?? [],
                        modelContexts: contexts[providerID] ?? [:]
                    ))
                }
                try await applyHandler(progress)
            }
        )
        let visibleFrame = NSScreen.main?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: 1_280, height: 800)
        let contentSize = providerSettingsContentSize(for: visibleFrame)
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: contentSize),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "模型与服务"
        window.minSize = NSSize(width: 960, height: 600)
        window.toolbarStyle = .unified
        configureOpaqueWindow(window)
        window.center()
        super.init(window: window)
        window.delegate = self
        benchmarkModel.providerListDidLoad = { [weak self] response, selectedID in
            guard let self else { return }
            self.model.codexInstallMode = response.codexInstallMode
            self.model.replaceProviders(
                response.providers,
                selecting: selectedID,
                preserveDrafts: true
            )
        }
        installContent()
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
        reloadProviders()
        beginBackgroundRefresh()
        Task { @MainActor [weak self] in
            await self?.benchmarkModel.refreshFromGatewayForPresentation()
        }
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard model.hasConnectionDrafts || model.hasPendingApplyRetry || benchmarkModel.dirty else {
            stopBackgroundRefresh()
            model.clearSensitiveDrafts()
            benchmarkModel.stopPolling()
            return true
        }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "还有未应用的更改"
        alert.informativeText = "应用当前服务商更改、放弃所有草稿并关闭，或继续编辑。"
        alert.addButton(withTitle: "应用当前更改")
        alert.addButton(withTitle: "放弃全部并关闭")
        alert.addButton(withTitle: "继续编辑")
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            applyChanges()
            return false
        case .alertSecondButtonReturn:
            model.discardAllDrafts()
            model.clearSensitiveDrafts()
            benchmarkModel.discardSelectionDrafts()
            stopBackgroundRefresh()
            benchmarkModel.stopPolling()
            return true
        default:
            return false
        }
    }

    func windowWillClose(_ notification: Notification) {
        stopBackgroundRefresh()
        model.clearSensitiveDrafts()
        benchmarkModel.stopPolling()
    }

    private func beginBackgroundRefresh() {
        stopBackgroundRefresh()
        let interval = backgroundRefreshIntervalNanoseconds
        backgroundRefreshTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                do {
                    try await Task.sleep(nanoseconds: interval)
                } catch {
                    return
                }
                guard let self else { return }
                guard !Task.isCancelled else { return }
                guard !model.isBusy, !benchmarkModel.isBusy else { continue }
                _ = try? await loadProvidersNow(
                    selecting: selectedProvider?.id,
                    preserveDrafts: true
                )
            }
        }
    }

    private func stopBackgroundRefresh() {
        backgroundRefreshTask?.cancel()
        backgroundRefreshTask = nil
    }

    private func installContent() {
        let rootView = ProviderSettingsRootView(
            model: model,
            benchmarkModel: benchmarkModel,
            onAdd: { [weak self] in self?.addProvider() },
            onRemove: { [weak self] in self?.removeProvider() },
            onToggle: { [weak self] in self?.toggleProvider() },
            onTest: { [weak self] in self?.testProvider() },
            onApplyChanges: { [weak self] in self?.applyChanges() },
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
              !model.isBusy,
              !benchmarkModel.isBusy
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
        guard !model.isBusy, !benchmarkModel.isBusy else { return }
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

    private func reloadProviders(
        selecting providerID: String? = nil,
        preserveDrafts: Bool = true
    ) {
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
                _ = try await loadProvidersNow(
                    selecting: providerID,
                    preserveDrafts: preserveDrafts
                )
            } catch {
                model.isBusy = false
                model.status = "读取失败"
                showAlert(title: "读取供应商失败", message: String(describing: error))
            }
        }
    }

    @discardableResult
    private func loadProvidersNow(
        selecting providerID: String?,
        preserveDrafts: Bool
    ) async throws -> ProviderListResponse {
        let previousID = providerID ?? selectedProvider?.id
        let loaded = try await loadHandler()
        model.codexInstallMode = loaded.codexInstallMode
        let selectedID = previousID.flatMap { candidate in
            loaded.providers.contains(where: { $0.id == candidate }) ? candidate : nil
        } ?? loaded.providers.first?.id
        model.replaceProviders(
            loaded.providers,
            selecting: selectedID,
            preserveDrafts: preserveDrafts
        )
        benchmarkModel.applyProviderList(loaded, selecting: selectedID)
        return loaded
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
        guard !model.isBusy, !benchmarkModel.isBusy, let window else { return }
        runAddProviderSheet(attachedTo: window) { [weak self] values in
            guard let self, let values else { return }
            submitNewProvider(values)
        }
    }

    private func submitNewProvider(_ values: AddProviderFormValues) {
        let id = nextProviderID(for: values.preset)
        let key = values.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        if values.preset != "aws-bedrock", key.isEmpty {
            showAlert(title: "缺少 API 密钥", message: "新增 Provider 必须填写 API 密钥。")
            return
        }
        var arguments = ["providers", "add", "--preset", values.preset, "--id", id]
        if values.preset == "aws-bedrock" {
            arguments.append(contentsOf: [
                "--aws-access-key-id", values.awsAccessKeyID,
                "--aws-secret-access-key", values.awsSecretAccessKey,
                "--aws-region", values.awsRegion,
            ])
            appendProviderArgument(&arguments, "--aws-session-token", values.awsSessionToken)
        } else {
            arguments.append(contentsOf: ["--key", key])
        }
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
        if values.preset == "baidu-oneapi" {
            appendBaiduAuthBridgeArguments(&arguments, mode: .disabled)
        }
        performMutation(
            arguments,
            status: "正在新增并发现模型 \(id)…",
            selecting: id,
            requiresBaiduBridge: nil
        )
    }

    func removeProvider() {
        guard let provider = selectedProvider, provider.kind == .configured,
              !model.isBusy, !benchmarkModel.isBusy
        else { return }
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
        guard let provider = selectedProvider, provider.kind == .configured,
              !model.isBusy, !benchmarkModel.isBusy
        else { return }
        let action = provider.enabled ? "disable" : "enable"
        performMutation(
            ["providers", action, provider.id],
            status: "正在\(provider.enabled ? "停用" : "启用") \(provider.id)…",
            selecting: provider.id
        )
    }

    func testProvider() {
        guard let provider = selectedProvider, provider.kind == .configured,
              !model.isBusy, !benchmarkModel.isBusy
        else { return }
        let selectedBridge = model.baiduAuthBridge
        var arguments = ["providers", "test", provider.id, "--json"]
        if provider.presetID == "aws-bedrock" {
            appendProviderArgument(&arguments, "--aws-access-key-id", model.awsAccessKeyID)
            appendProviderArgument(&arguments, "--aws-secret-access-key", model.awsSecretAccessKey)
            appendProviderArgument(&arguments, "--aws-session-token", model.awsSessionToken)
            appendProviderArgument(&arguments, "--aws-region", model.awsRegion)
        } else {
            appendProviderArgument(&arguments, "--key", model.apiKey)
        }
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
              !benchmarkModel.isBusy,
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
            [
                "providers", "update", provider.id,
                provider.presetID == "aws-bedrock" ? "--clear-aws-credentials" : "--clear-key",
            ],
            status: "正在清除 \(provider.id) 的密钥…",
            selecting: provider.id
        )
    }

    func clearQuotaCredentials() {
        guard let provider = selectedProvider, provider.kind == .configured,
              !model.isBusy, !benchmarkModel.isBusy
        else { return }
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
        guard let provider = selectedProvider, provider.kind == .configured,
              !model.isBusy, !benchmarkModel.isBusy
        else { return }
        guard let plan = providerUpdatePlan(for: provider) else { return }
        performMutation(
            plan.arguments,
            status: "正在保存 \(provider.id)…",
            selecting: provider.id,
            requiresBaiduBridge: plan.requiresBaiduBridge,
            codexSkillChanged: plan.codexSkillChanged
        )
    }

    func applyChanges() {
        guard let provider = selectedProvider, !model.isBusy, !benchmarkModel.isBusy else { return }
        let updatePlan: ProviderUpdatePlan?
        if model.connectionDirty, provider.kind == .configured {
            guard let plan = providerUpdatePlan(for: provider) else { return }
            updatePlan = plan
        } else {
            updatePlan = nil
        }
        let selectionUpdate = benchmarkModel.selectionUpdate(for: provider.id)
        guard updatePlan != nil || selectionUpdate != nil || model.applyRetryRequired else { return }

        setBusy(true, status: "正在应用 \(provider.displayName) 的更改…")
        Task { @MainActor [weak self] in
            guard let self else { return }
            var configurationWritten = model.applyRetryRequired
            do {
                try await runOperationProgress(
                    title: "正在应用模型与服务设置",
                    phases: [
                        "验证当前服务商更改",
                        "写入连接设置和模型选择",
                        "重启本地网关",
                        "刷新 Codex 模型目录",
                        "完成",
                    ],
                    successTitle: "✓ 更改已应用",
                    failureTitle: "✗ 应用失败",
                    showFailureAlert: false
                ) { progress in
                    progress.advance(to: 0)
                    if var updatePlan {
                        if let mode = updatePlan.requiresBaiduBridge, mode != .disabled {
                            let executable = try await self.ensureBaiduBridgeAvailable(mode)
                            appendBaiduAuthBridgeExecutable(
                                &updatePlan.arguments,
                                mode: mode,
                                executable: executable
                            )
                        }
                        _ = try await self.runHandler(updatePlan.arguments)
                        configurationWritten = true
                    }
                    if let selectionUpdate {
                        try await self.saveModelSelectionHandler(selectionUpdate)
                        configurationWritten = true
                    }
                    progress.advance(to: 1)
                    try await self.applyHandler(progress)
                    progress.advance(to: 4)
                }
                if selectionUpdate != nil {
                    benchmarkModel.markSelectionSaved(for: provider.id)
                }
                if model.applyRetryProviderID == provider.id {
                    model.applyRetryProviderID = nil
                }
                do {
                    _ = try await loadProvidersNow(
                        selecting: provider.id,
                        preserveDrafts: false
                    )
                } catch {
                    setBusy(false, status: "更改已应用，界面刷新失败")
                    showBanner(
                        title: "更改已应用",
                        message: "重新读取服务商列表失败：\(localizedErrorDescription(error))",
                        isError: true
                    )
                    return
                }
                setBusy(false, status: "更改已应用")
                showBanner(
                    title: "更改已应用",
                    message: "Codex 模型目录已更新；运行中的 Codex 需重启后读取新列表。",
                    isError: false
                )
            } catch {
                if configurationWritten {
                    model.applyRetryProviderID = provider.id
                }
                setBusy(false, status: "应用失败")
                showBanner(
                    title: configurationWritten ? "配置已保存，应用未完成" : "应用失败",
                    message: configurationWritten
                        ? "重启网关或刷新 Codex 模型目录失败：\(localizedErrorDescription(error))"
                        : localizedErrorDescription(error),
                    isError: true
                )
                reloadProviders(selecting: provider.id)
            }
        }
    }

    private func providerUpdatePlan(for provider: ProviderView) -> ProviderUpdatePlan? {
        let auxiliaryModelUpstream = model.auxiliaryModelUpstream
        var update = ["providers", "update", provider.id]
        update.append("--auxiliary-model-upstream")
        update.append(auxiliaryModelUpstream ? "true" : "false")
        if provider.presetID == "aws-bedrock" {
            let region = model.awsRegion.trimmingCharacters(in: .whitespacesAndNewlines)
            let hasStoredCredentials = provider.awsSigV4Configured == true
            let accessKeyID = model.awsAccessKeyID.trimmingCharacters(in: .whitespacesAndNewlines)
            let secretAccessKey = model.awsSecretAccessKey.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !region.isEmpty,
                  hasStoredCredentials || (!accessKeyID.isEmpty && !secretAccessKey.isEmpty)
            else {
                showAlert(
                    title: "缺少 AWS 凭据",
                    message: "Amazon Bedrock 必须填写 Region、Access Key ID 和 Secret Access Key。"
                )
                return nil
            }
            appendProviderArgument(&update, "--aws-access-key-id", accessKeyID)
            appendProviderArgument(&update, "--aws-secret-access-key", secretAccessKey)
            appendProviderArgument(&update, "--aws-session-token", model.awsSessionToken)
            appendProviderArgument(&update, "--aws-region", region)
            if model.clearAwsSessionToken {
                update.append("--clear-aws-session-token")
            }
        } else {
            appendProviderArgument(&update, "--key", model.apiKey)
        }
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
                return nil
            }
            appendProviderArgument(&update, "--display-name", displayName)
            appendProviderArgument(&update, "--base-url", baseURL)
            update.append(contentsOf: [
                "--protocol", model.protocolID,
                "--api-path", apiPath(for: model.protocolID),
            ])
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
            return nil
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
                    return nil
                }
                appendProviderArgument(&update, "--quota-workspace-id", workspaceID)
                appendProviderArgument(&update, "--quota-auth-cookie", authCookie)
            }
        }
        return ProviderUpdatePlan(
            arguments: update,
            requiresBaiduBridge: provider.presetID == "baidu-oneapi"
                && baiduBridgeNeedsSetup(
                    current: provider.effectiveBaiduAuthBridge,
                    selected: selectedBaiduBridge
                ) ? selectedBaiduBridge : nil,
            codexSkillChanged: auxiliaryModelUpstream != provider.auxiliaryModelUpstream
        )
    }

    private func apiPath(for protocolID: String) -> String {
        switch protocolID {
        case "anthropic_messages": return "/v1/messages"
        case "open_ai_chat": return "/v1/chat/completions"
        default: return "/v1/responses"
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
        guard !model.isBusy, !benchmarkModel.isBusy else { return }
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
                let loaded = try await loadProvidersNow(
                    selecting: providerID,
                    preserveDrafts: false
                )
                setBusy(false, status: "配置已保存")
                let addedProvider = Array(initialArguments.prefix(2)) == ["providers", "add"]
                    ? loaded.providers.first { $0.id == providerID }
                    : nil
                let message: String
                if let addedProvider {
                    message = "已按现有规则加入 \(addedProvider.selectedModels.count) 个模型，可在下方调整后应用。"
                } else if codexSkillChanged {
                    message = "生图 Skill 已更新。请重启 Codex 后新建线程，使 Codex 重新加载 Skill。"
                } else {
                    message = AppLocalization.string("providerSettings.theCodexModelCatalogHasBeenRegenerated")
                }
                showBanner(
                    title: AppLocalization.string("providerSettings.providerConfigurationUpdated"),
                    message: message,
                    isError: false
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
