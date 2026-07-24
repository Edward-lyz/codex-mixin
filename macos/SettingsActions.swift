import Cocoa

extension AppDelegate {
    @objc func runAutomaticDoctor() {
        guard !serviceBusy else { return }
        serviceBusy = true
        serviceStatus = "正在自动检测..."
        Task { @MainActor in
            defer {
                serviceBusy = false
                Task { @MainActor in
                    await self.refreshStatusNow()
                }
            }
            do {
                let report = try await runGateway(["doctor"])
                appendDiagnosticLog("Automatic doctor report\n\(report)")
                showDiagnosticReport(title: "Codex Mixin 自动检测", report: report)
            } catch {
                showAlert(title: "自动检测失败", message: String(describing: error))
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
                applyHandler: { [weak self] in
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
                        return
                    }
                    try await self.restartGatewayProcess()
                    let status = try await self.waitForGatewayStatus()
                    self.applyGatewayStatus(status)
                    _ = try await self.runGateway(["refresh-codex-catalog"])
                    await self.refreshStatusNow()
                }
            )
        }
        providerSettingsWindowController?.present()
    }

    @objc func showModelBenchmark() {
        if modelBenchmarkWindowController == nil {
            modelBenchmarkWindowController = ModelBenchmarkWindowController(
                snapshotURL: stateDir().appendingPathComponent("model-benchmarks.json"),
                startHandler: { [weak self] timeoutSeconds, providerID in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    let status = try await self.ensureGatewayReady()
                    self.applyGatewayStatus(status)
                    guard let snapshot = try await self.modelBenchmarkRequest(
                        method: "POST",
                        timeoutSeconds: timeoutSeconds,
                        providerID: providerID
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
                        providerID: nil
                    )
                },
                providerOptionsHandler: { [weak self] in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    let response = try decodeProviderList(
                        try await self.runGateway(["providers", "list", "--json"])
                    )
                    return response.providers
                        .filter(\.enabled)
                        .map {
                            BenchmarkProviderOption(id: $0.id, displayName: $0.displayName)
                        }
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
                saveHandler: { [weak self] profile, replacedProfileID in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    var arguments = [
                        "fusion",
                        "set",
                        "--profile-json",
                        try profile.jsonString(),
                    ]
                    arguments.append(contentsOf: ["--replace-id", replacedProfileID])
                    _ = try await self.runGateway(arguments)
                    self.serviceBusy = true
                    self.serviceStatus = "正在应用 Fusion 配置..."
                    self.serviceEndpoint = nil
                    defer { self.serviceBusy = false }
                    try await self.restartGatewayProcess()
                    let status = try await self.waitForGatewayStatus()
                    self.applyGatewayStatus(status)
                    _ = try await self.runGateway(["refresh-codex-catalog"])
                    await self.refreshStatusNow()
                }
            )
        }
        fusionSettingsWindowController?.present()
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
        providerID: String?
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
