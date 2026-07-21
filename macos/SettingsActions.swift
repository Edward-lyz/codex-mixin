import Cocoa

extension AppDelegate {
    @objc func configureLogin() {
        let stored: [String: Any]
        do {
            stored = try loadStoredConfig()
        } catch {
            showAlert(title: "读取配置失败", message: String(describing: error))
            return
        }
        guard let values = runLoginSettingsPanel(stored: stored) else { return }
        let apiKey = values.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        if apiKey.isEmpty && stored["upstream_api_key"] == nil {
            showAlert(title: "缺少 API 密钥", message: "首次配置必须填写上游 API 密钥。")
            return
        }
        if values.provider == "custom" && values.baseUrl.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && stored["upstream_base_url"] == nil {
            showAlert(title: "缺少上游地址", message: "Custom 模式必须填写上游服务根地址。")
            return
        }
        var args = ["login"]
        appendOptionalArg(&args, "--provider", values.provider)
        appendOptionalArg(&args, "--key", apiKey)
        appendOptionalArg(&args, "--base-url", values.baseUrl)
        args.append("--image-generation-path")
        args.append(values.imageGenerationPath.trimmingCharacters(in: .whitespacesAndNewlines))
        appendOptionalArg(&args, "--gateway-key", values.gatewayKey)
        appendOptionalArg(&args, "--quota-url", values.quotaUrl)
        appendOptionalArg(&args, "--quota-username", values.quotaUsername)
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在应用新配置..."
            serviceEndpoint = nil
            defer { serviceBusy = false }
            do {
                let output = try await runGateway(args)
                try await restartGatewayProcess()
                let status = try await waitForGatewayStatus()
                applyGatewayStatus(status)
                _ = try await runGateway(["refresh-codex-catalog"])
                showAlert(title: "设置已保存", message: output.isEmpty ? "完成" : output)
                await refreshStatusNow()
            } catch {
                serviceStatus = "应用配置失败"
                showAlert(title: "保存设置失败", message: String(describing: error))
            }
        }
    }

    @objc func showModelBenchmark() {
        if modelBenchmarkWindowController == nil {
            modelBenchmarkWindowController = ModelBenchmarkWindowController(
                snapshotURL: stateDir().appendingPathComponent("model-benchmarks.json"),
                startHandler: { [weak self] timeoutSeconds in
                    guard let self else {
                        throw GatewayError.command("Codex Mixin 已退出")
                    }
                    let status = try await self.ensureGatewayReady()
                    self.applyGatewayStatus(status)
                    guard let snapshot = try await self.modelBenchmarkRequest(
                        method: "POST",
                        timeoutSeconds: timeoutSeconds
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
                    return try await self.modelBenchmarkRequest(method: "GET", timeoutSeconds: nil)
                }
            )
        }
        modelBenchmarkWindowController?.present()
    }

    @objc func showFusionSettings() {
        if fusionSettingsWindowController == nil {
            fusionSettingsWindowController = FusionSettingsWindowController(
                loadHandler: {
                    FusionSettingsProfile.fromStoredConfig(try loadStoredConfig())
                },
                fetchModelsHandler: { [weak self] in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    return try await self.fetchFusionModelOptions()
                },
                saveHandler: { [weak self] profile in
                    guard let self else {
                        throw FusionSettingsError.message("Codex Mixin 已退出")
                    }
                    try saveFusionProfile(profile)
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
        guard
            let endpoint = serviceEndpoint,
            let url = URL(string: "\(endpoint)/models")
        else {
            throw FusionSettingsError.message("本地网关未运行，请先从菜单启动本地网关。")
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = 15
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let stored = try loadStoredConfig()
        let bearer = stored["gateway_api_key"] as? String ?? "codex-mixin-menu"
        request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw FusionSettingsError.message("模型接口没有返回 HTTP 状态")
        }
        guard (200..<300).contains(httpResponse.statusCode) else {
            let body = String(data: data, encoding: .utf8) ?? "HTTP \(httpResponse.statusCode)"
            throw FusionSettingsError.message("读取模型失败：\(body)")
        }
        guard
            let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let models = object["data"] as? [[String: Any]]
        else {
            throw FusionSettingsError.message("模型接口返回了无效 JSON")
        }
        let provider = stored["provider_preset"] as? String ?? "custom"
        let upstream: [FusionModelOption] = models.compactMap { model -> FusionModelOption? in
            guard
                let id = model["id"] as? String,
                !id.hasPrefix("mixin/fusion/")
            else { return nil }
            return FusionModelOption(
                id: "\(provider):\(id)",
                displayName: "\(model["display_name"] as? String ?? id) · \(provider)"
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

    func modelBenchmarkRequest(method: String, timeoutSeconds: Int?) async throws -> ModelBenchmarkSnapshot? {
        guard
            let endpoint = serviceEndpoint,
            let url = URL(string: "\(endpoint)/model-benchmarks")
        else {
            throw GatewayError.command("本地网关尚未就绪")
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = TimeInterval((timeoutSeconds ?? 0) + 5)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let stored = try loadStoredConfig()
        let bearer = stored["gateway_api_key"] as? String ?? "codex-mixin-menu"
        request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        if let timeoutSeconds {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: [
                "timeout_seconds": timeoutSeconds,
            ])
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse else {
            throw GatewayError.command("测速接口没有返回 HTTP 状态")
        }
        guard (200..<300).contains(httpResponse.statusCode) else {
            let message = ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any])
                .flatMap { $0["error"] as? [String: Any] }?
                .flatMap { $0["message"] as? String }
                ?? String(data: data, encoding: .utf8)
                ?? "HTTP \(httpResponse.statusCode)"
            throw GatewayError.command(message)
        }
        return try JSONDecoder().decode(ModelBenchmarkSnapshotEnvelope.self, from: data).snapshot
    }
}
