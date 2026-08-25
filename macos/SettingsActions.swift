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
                    let providers = try decodeProviderList(
                        try await self.runGateway(["providers", "list", "--json"])
                    )
                    if providers.providers.isEmpty {
                        progress?.advance(to: 1)
                        if FileManager.default.fileExists(atPath: self.launchAgentPath().path) {
                            try await self.bootoutIfLoaded(self.launchDomainAndLabel())
                        }
                        _ = try await self.runGateway(["stop"])
                        try await self.waitForGatewayStopped()
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
                    progress?.advance(to: 1)
                    try await self.restartGatewayProcess()
                    let status = try await self.waitForGatewayStatus()
                    self.applyGatewayStatus(status)
                    progress?.advance(to: 2)
                    _ = try await self.runGateway(["refresh-codex-catalog"])
                    await self.refreshStatusNow()
                    progress?.advance(to: 3)
                }
            )
        }
        providerSettingsWindowController?.present()
    }

    @objc func showModelBenchmark() {
        if modelBenchmarkWindowController == nil {
            modelBenchmarkWindowController = ModelBenchmarkWindowController(
                startHandler: { [weak self] timeoutSeconds, providerID, targetOutputTokens in
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
                fetchHandler: { [weak self] in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    if self.serviceEndpoint == nil,
                       let status = try? await self.runGateway(["status"])
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
                loadProvidersHandler: { [weak self] in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    return try decodeProviderList(
                        try await self.runGateway(["providers", "list", "--json"])
                    )
                },
                saveSelectionsHandler: { [weak self] selections, progress in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    progress.advance(to: 0)
                    for providerID in selections.keys.sorted() {
                        var arguments = ["providers", "select", providerID]
                        for modelID in selections[providerID] ?? [] {
                            arguments.append(contentsOf: ["--model", modelID])
                        }
                        _ = try await self.runGateway(arguments)
                    }
                    self.serviceBusy = true
                    self.serviceStatus = "正在应用模型选择..."
                    self.serviceEndpoint = nil
                    defer { self.serviceBusy = false }
                    progress.advance(to: 1)
                    try await self.restartGatewayProcess()
                    let status = try await self.waitForGatewayStatus()
                    self.applyGatewayStatus(status)
                    progress.advance(to: 2)
                    _ = try await self.runGateway(["refresh-codex-catalog"])
                    await self.refreshStatusNow()
                    progress.advance(to: 3)
                },
                discoverHandler: { [weak self] providerID, onProgress in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    _ = try await self.runGatewayStreaming(
                        ["providers", "discover", providerID],
                        onProgress: onProgress
                    )
                },
                probeHandler: { [weak self] providerID, onProgress in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    _ = try await self.runGatewayStreaming(
                        ["providers", "probe", providerID],
                        onProgress: onProgress
                    )
                }
            )
        }
        modelBenchmarkWindowController?.present()
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
        serviceStatus = "正在准备 DUCX 全量上报..."
        serviceEndpoint = nil
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在手动上报本地 Session",
                    phases: [
                        "清除旧上报凭据",
                        "执行 DUCX warmup",
                        "扫描并重放本地 Session",
                        "完成",
                    ],
                    successTitle: "✓ 本地 Session 上报完成",
                    failureTitle: "✗ 本地 Session 上报失败",
                    showFailureAlert: true,
                    failureAlertTitle: "手动触发上报失败"
                ) { progress in
                    progress.advance(to: 0)
                    _ = try await runGateway(["report-replay", "--prepare-warmup"])
                    progress.advance(to: 1)
                    try await restartGatewayProcess()
                    let status = try await waitForGatewayStatus()
                    applyGatewayStatus(status)
                    progress.advance(to: 2)
                    let report = try await runGateway(["report-replay", "--all-sessions"])
                    appendDiagnosticLog("Manual DUCX report replay\n\(report)")
                    progress.advance(to: 3)
                    await refreshStatusNow()
                }
            } catch {
                await refreshStatusNow()
            }
        }
    }

    func applyFusionCatalogChange(progress: OperationProgress) async throws {
        serviceBusy = true
        serviceStatus = "正在应用 Fusion 配置..."
        serviceEndpoint = nil
        defer { serviceBusy = false }
        progress.advance(to: 1)
        try await restartGatewayProcess()
        let status = try await waitForGatewayStatus()
        applyGatewayStatus(status)
        progress.advance(to: 2)
        _ = try await runGateway(["refresh-codex-catalog"])
        await refreshStatusNow()
        progress.advance(to: 3)
    }

    func fetchFusionModelOptions() async throws -> [FusionModelOption] {
        let data = Data(try await runGateway(["models", "--json"]).utf8)
        guard
            let models = try JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else {
            throw FusionSettingsError.message("模型接口返回了无效 JSON")
        }
        let upstream: [FusionModelOption] = models.compactMap { model -> FusionModelOption? in
            guard
                let id = model["id"] as? String,
                !id.hasPrefix("mixin/fusion/")
            else { return nil }
            return FusionModelOption(
                id: id,
                displayName: model["display_name"] as? String ?? id
            )
        }
        let official = try loadOfficialFusionModelOptions()
        return (official + upstream).sorted {
            $0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending
        }
    }

    func loadOfficialFusionModelOptions() throws -> [FusionModelOption] {
        let cacheURL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".codex/models_cache.json")
        guard FileManager.default.fileExists(atPath: cacheURL.path) else { return [] }
        let data = try Data(contentsOf: cacheURL)
        guard
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let models = object["models"] as? [[String: Any]]
        else {
            throw FusionSettingsError.message("OpenAI 官方模型缓存格式无效")
        }
        return models.compactMap { model -> FusionModelOption? in
            guard
                let slug = model["slug"] as? String,
                (model["visibility"] as? String ?? "list") != "hide"
            else { return nil }
            return FusionModelOption(
                id: "official:\(slug)",
                displayName: "\(model["display_name"] as? String ?? slug) · OpenAI 官方"
            )
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
