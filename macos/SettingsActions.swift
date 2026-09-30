import Cocoa

extension AppDelegate {
    @objc func showAbout() {
        aboutWindowController?.close()
        let controller = AboutWindowController(showCard: { [weak self] wallpaperOffset in
            self?.showInstallCard(wallpaperOffset: wallpaperOffset)
        })
        aboutWindowController = controller
        controller.present()
    }

    func showInstallCard(wallpaperOffset: Int) {
        installCardWindowController?.close()
        let controller = InstallCardWindowController(wallpaperOffset: wallpaperOffset)
        installCardWindowController = controller
        controller.present()
    }

    @objc func runAutomaticDoctor() {
        guard !serviceBusy, !automaticDoctorBusy else { return }
        automaticDoctorBusy = true
        serviceBusy = true
        serviceStatus = "正在健康检测和修复..."
        Task { @MainActor in
            defer {
                automaticDoctorBusy = false
                serviceBusy = false
            }
            do {
                try await runOperationProgress(
                    title: "正在健康检测和修复",
                    phases: [
                        "运行 doctor --fix --quick",
                        "刷新状态",
                        "完成",
                    ],
                    successTitle: "✓ 检测完成",
                    failureTitle: "✗ 检测失败",
                    showFailureAlert: true,
                    failureAlertTitle: "健康检测和修复失败"
                ) { progress in
                    progress.advance(to: 0)
                    let report = try await runGateway(["doctor", "--fix", "--quick"])
                    appendDiagnosticLog("Health check and repair report\n\(report)")
                    progress.advance(to: 1)
                    await refreshStatusNow()
                    progress.advance(to: 2)
                    showDiagnosticReport(title: "Codex Mixin 健康检测和修复", report: report)
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func configureLogin() {
        if providerSettingsWindowController == nil {
            providerSettingsWindowController = ProviderSettingsWindowController(
                loadHandler: { [weak self] in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    return try decodeProviderList(
                        try await self.runGateway(["providers", "list", "--json"])
                    )
                },
                runHandler: { [weak self] arguments in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    return try await self.runGateway(arguments)
                },
                applyHandler: { [weak self] progress in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    self.serviceBusy = true
                    self.serviceStatus = "正在应用 Provider 配置..."
                    self.serviceEndpoint = nil
                    defer { self.serviceBusy = false }
                    progress?.advance(to: 1)
                    _ = try await self.runGateway(["config", "apply"])
                    let providers = try decodeProviderList(
                        try await self.runGateway(["providers", "list", "--json"])
                    )
                    if providers.providers.isEmpty {
                        self.isRunning = false
                        self.serviceStatus = "等待配置上游 API"
                        self.serviceEndpoint = nil
                        self.updateQuotaStatus(
                            title: "额度：等待配置",
                            detail: nil,
                            progress: nil
                        )
                        self.updateStatusTitle()
                        self.updateActionStates()
                        progress?.advance(to: 3)
                        return
                    }
                    progress?.advance(to: 2)
                    await self.refreshStatusNow()
                    progress?.advance(to: 3)
                },
                benchmarkStartHandler: { [weak self] timeoutSeconds, providerID, targetOutputTokens in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    let status = try await self.ensureGatewayReady()
                    self.applyGatewayStatus(status)
                    guard let snapshot = try await self.modelBenchmarkRequest(
                        method: "POST",
                        timeoutSeconds: timeoutSeconds,
                        providerID: providerID,
                        targetOutputTokens: targetOutputTokens
                    ) else {
                        throw GatewayError.command("网关未返回测速任务")
                    }
                    return snapshot
                },
                benchmarkFetchHandler: { [weak self] in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    if self.serviceEndpoint == nil,
                       let status = try? await self.runGateway(["status", "--json"])
                    {
                        self.applyGatewayStatus(status)
                    }
                    return try await self.modelBenchmarkRequest(
                        method: "GET",
                        timeoutSeconds: nil,
                        providerID: nil,
                        targetOutputTokens: nil
                    )
                },
                saveModelSelectionHandler: { [weak self] update in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    var arguments = ["providers", "select", update.providerID]
                    for modelID in update.modelIDs {
                        arguments.append(contentsOf: ["--model", modelID])
                    }
                    for (modelID, contextWindow) in update.modelContexts.sorted(by: {
                        $0.key.localizedStandardCompare($1.key) == .orderedAscending
                    }) {
                        arguments.append(contentsOf: [
                            "--model-context", "\(modelID)=\(contextWindow)",
                        ])
                    }
                    for modelID in update.clearedModelContexts {
                        arguments.append(contentsOf: ["--clear-model-context", modelID])
                    }
                    _ = try await self.runGateway(arguments)
                }
            )
        }
        providerSettingsWindowController?.present()
    }

    @objc func showModelBenchmark() {
        configureLogin()
    }

    @objc func configureEchAccess() {
        guard !serviceBusy else { return }
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                let output = try await runGateway(["ech", "status", "--json"])
                guard let data = output.data(using: .utf8),
                      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let enabled = object["enabled"] as? Bool
                else { throw GatewayError.command("ECH 状态接口返回了无效 JSON") }
                NSApp.activate(ignoringOtherApps: true)
                let alert = NSAlert()
                alert.messageText = "启用 ECH 代理访问 GPT"
                alert.informativeText = "当前：\(enabled ? "已启用" : "未启用")\n仅接管本地网关的官方 GPT 请求。使用 edge.1molchuan.top/plus 的 IPv4 和 ECH 配置，保留端到端 TLS。ECH 连接失败会自动关闭并回退到直连。\n\n启用会先测试连接，再保存设置并重启网关。ECH 成功不能证明中继出口位于 Azure；不影响浏览器登录或第三方 Provider。"
                if let reason = object["fallback_reason"] as? String {
                    alert.informativeText += "\n\n已自动回退到直连：\(reason)"
                }
                alert.addButton(withTitle: enabled ? "关闭并应用" : "启用并测试")
                alert.addButton(withTitle: "仅测试连接")
                alert.addButton(withTitle: "取消")
                let choice = alert.runModal()
                guard choice != .alertThirdButtonReturn else { return }
                let command = choice == .alertSecondButtonReturn ? "test" : (enabled ? "disable" : "enable")
                try await runOperationProgress(
                    title: "ECH 代理访问 GPT",
                    phases: ["检查并应用 ECH 连接", "刷新服务状态"],
                    showFailureAlert: false,
                    failureAlertTitle: "ECH 操作失败"
                ) { progress in
                    progress.advance(to: 0)
                    let report = try await runGateway(["ech", command])
                    progress.advance(to: 1)
                    await refreshStatusNow()
                    showDiagnosticReport(title: "ECH 代理访问 GPT", report: report)
                }
            } catch {
                showAlert(title: "ECH 操作失败", message: String(describing: error))
            }
        }
    }

    @objc func showFusionSettings() {
        if fusionSettingsWindowController == nil {
            fusionSettingsWindowController = FusionSettingsWindowController(
                loadHandler: { [weak self] in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    return try FusionSettingsProfile.fromCLIJSON(
                        try await self.runGateway(["fusion", "get", "--json"])
                    )
                },
                fetchModelsHandler: { [weak self] in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    return try await self.fetchFusionModelOptions()
                },
                saveHandler: { [weak self] profile, replacedProfileID, progress in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    progress.advance(to: 0)
                    var arguments = [
                        "fusion",
                        "set",
                        "--profile-json",
                        try profile.jsonString(),
                    ]
                    arguments.append(contentsOf: ["--replace-id", replacedProfileID])
                    _ = try await self.runGateway(arguments)
                    try await self.applyFusionCatalogChange(progress: progress)
                },
                deleteHandler: { [weak self] profileID, progress in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    progress.advance(to: 0)
                    _ = try await self.runGateway(["fusion", "delete", "--id", profileID])
                    try await self.applyFusionCatalogChange(progress: progress)
                }
            )
        }
        fusionSettingsWindowController?.present()
    }

    @objc func manuallyReportSessions() {
        guard !serviceBusy else { return }
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            var replayStarted = false
            do {
                let estimate = try decodeDUCXReplayEstimate(
                    try await runGateway([
                        "report-replay",
                        "--all-sessions",
                        "--estimate",
                        "--json",
                    ])
                )
                guard confirm(
                    title: "手动上报全部本地 Session？",
                    message: "预计上报 \(estimate.estimatedSessions) 个会话，共 \(estimate.estimatedReports) 次。此功能仅用于自动上报数据异常时的修复。确认异常确实需要修复后再继续。"
                ) else { return }
                replayStarted = true
                serviceStatus = "正在准备 DUCX 全量上报..."
                serviceEndpoint = nil
                let replayReport = try await runOperationProgress(
                    title: "正在手动上报本地 Session",
                    phases: [
                        "清除旧上报凭据",
                        "执行 DUCX warmup",
                        "扫描并重放本地 Session",
                        "完成",
                    ],
                    successTitle: "✓ 本地 Session 上报已处理",
                    failureTitle: "✗ 本地 Session 上报失败",
                    showFailureAlert: true,
                    failureAlertTitle: "手动触发上报失败"
                ) { progress in
                    progress.advance(to: 0)
                    _ = try await runGateway(["report-replay", "--prepare-warmup"])
                    progress.advance(to: 1)
                    let status = try await runGateway(["service", "restart", "--managed", "--json"])
                    applyGatewayStatus(status)
                    progress.advance(to: 2)
                    let report = try await runGateway([
                        "report-replay",
                        "--all-sessions",
                        "--json",
                    ])
                    appendDiagnosticLog("Manual DUCX report replay\n\(report)")
                    let replayReport = try decodeDUCXReplayReport(report)
                    progress.advance(to: 3)
                    await refreshStatusNow()
                    return replayReport
                }
                showDiagnosticReport(
                    title: "DUCX 手动上报结果",
                    report: formatDUCXReplayReport(replayReport)
                )
            } catch {
                await refreshStatusNow()
                if !replayStarted {
                    showAlert(title: "手动上报预估失败", message: error.localizedDescription)
                }
            }
        }
    }

    func applyFusionCatalogChange(progress: OperationProgress) async throws {
        serviceBusy = true
        serviceStatus = "正在应用 Fusion 配置..."
        serviceEndpoint = nil
        defer { serviceBusy = false }
        progress.advance(to: 1)
        _ = try await runGateway(["config", "apply"])
        progress.advance(to: 2)
        await refreshStatusNow()
        progress.advance(to: 3)
    }

    func fetchFusionModelOptions() async throws -> [FusionModelOption] {
        let data = Data(try await runGateway(["fusion", "models", "--json"]).utf8)
        guard
            let response = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let models = response["models"] as? [[String: Any]]
        else {
            throw FusionSettingsError.message("模型接口返回了无效 JSON")
        }
        return models.compactMap { model -> FusionModelOption? in
            guard
                let id = model["id"] as? String,
                !id.hasPrefix("mixin/fusion/")
            else { return nil }
            return FusionModelOption(
                id: id,
                displayName: model["display_name"] as? String ?? id
            )
        }.sorted {
            $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending
        }
    }

    func modelBenchmarkRequest(
        method: String,
        timeoutSeconds: Int?,
        providerID: String?,
        targetOutputTokens: Int?
    ) async throws -> ModelBenchmarkSnapshot? {
        let output: String
        if method == "POST", let timeoutSeconds {
            var arguments = [
                "benchmark",
                "start",
                "--timeout-seconds",
                String(timeoutSeconds),
            ]
            if let providerID {
                arguments.append(contentsOf: ["--provider", providerID])
            }
            if let targetOutputTokens {
                arguments.append(contentsOf: [
                    "--target-output-tokens",
                    String(targetOutputTokens),
                ])
            }
            output = try await runGateway(arguments)
        } else {
            output = try await runGateway(["benchmark", "status"])
        }
        return try JSONDecoder().decode(
            ModelBenchmarkSnapshotEnvelope.self,
            from: Data(output.utf8)
        ).snapshot
    }
}
