import Cocoa

private struct GatewayHealthResponse: Decodable {
    let ok: Bool
    let providerReadiness: String?

    enum CodingKeys: String, CodingKey {
        case ok
        case providerReadiness = "provider_readiness"
    }
}

extension AppDelegate {
    func applyGatewayStatus(_ status: String?) {
        guard let status else {
            _ = applyGatewayStatusFailure(GatewayError.command("网关状态响应为空"))
            return
        }
        do {
            applyGatewayStatus(try decodeGatewayStatus(status))
        } catch {
            isRunning = false
            serviceEndpoint = nil
            providerStatusDetail = localizedErrorDescription(error)
            serviceStatus = "网关状态检查失败"
            updateStatusTitle()
            updateActionStates()
        }
    }

    private func applyGatewayStatus(_ snapshot: GatewayStatusSnapshot) {
        echMenuItem?.state = snapshot.officialEch?.enabled == true ? .on : .off
        echFallbackReason = snapshot.officialEch?.fallback_reason
        if let reason = echFallbackReason, reason != presentedEchFallbackReason {
            presentedEchFallbackReason = reason
            showAlert(title: "ECH 已自动关闭", message: "官方请求已回退到直连。\n\n\(reason)")
        } else if echFallbackReason == nil {
            presentedEchFallbackReason = nil
        }
        if snapshot.configured == false {
            isRunning = false
            serviceEndpoint = nil
            providerStatusDetail = nil
            serviceStatus = "等待配置上游 API"
            updateStatusTitle()
            updateActionStates()
            return
        }
        isRunning = snapshot.gateway == "running"
        serviceEndpoint = snapshot.endpoint
        let problemProviders = snapshot.providers?.filter {
            $0.enabled && $0.readiness.status == "degraded"
        } ?? []
        notifyOfficialModelRefreshFailure(snapshot.providers ?? [])
        let issueDetails = gatewayProviderIssueDetails(problemProviders)
        providerStatusDetail = issueDetails.isEmpty ? nil : issueDetails.joined(separator: "；")
        if isRunning, !problemProviders.isEmpty {
            serviceStatus = "本地服务运行中 · \(problemProviders.count) 个服务商需要处理"
        } else if isRunning, snapshot.providerReadiness == "disabled" {
            serviceStatus = "本地服务运行中 · 无启用服务商"
        } else {
            serviceStatus = isRunning
                ? "本地服务运行中"
                : stoppedGatewayStatusTitle(codexStatus: codexIntegrationStatus)
        }
        if let reason = echFallbackReason {
            serviceStatus += " · ECH 已回退直连"
            providerStatusDetail = [providerStatusDetail, reason].compactMap { $0 }.joined(separator: "；")
        }
        updateStatusTitle()
        updateActionStates()
    }

    private func notifyOfficialModelRefreshFailure(_ providers: [GatewayStatusProvider]) {
        let failures = providers.compactMap { provider -> String? in
            guard provider.id == "official",
                  provider.enabled,
                  provider.readiness.issues.contains("model_refresh_failed")
            else {
                return nil
            }
            return provider.readiness.lastModelRefreshError ?? "未知错误"
        }
        let activeKeys: Set<String> = failures.isEmpty ? [] : ["official:error"]
        presentedModelRefreshFailureKeys.formIntersection(activeKeys)
        guard let error = failures.first,
              let key = activeKeys.first,
              presentedModelRefreshFailureKeys.insert(key).inserted
        else {
            return
        }
        DispatchQueue.main.async { [weak self] in
            guard self != nil else { return }
            showAlert(
                title: "官方模型列表刷新失败",
                message: "暂时继续使用上次成功获取的模型列表。网关会在后台自动重试。\n\n\(error)"
            )
        }
    }

    func refreshStatusNow() async {
        await requestStatusRefresh(scope: .full)
    }

    func refreshMenuStatus() async {
        await requestStatusRefresh(scope: .status)
    }

    func refreshScheduledStatus() async {
        let scope: StatusRefreshScope = quotaRefreshPolicy.isDue() ? .status : .health
        await requestStatusRefresh(scope: scope)
    }

    private func requestStatusRefresh(scope: StatusRefreshScope) async {
        pendingStatusRefreshScope = pendingStatusRefreshScope?.merged(with: scope) ?? scope
        await statusRefreshCoordinator.refresh()
    }

    @MainActor
    func performStatusRefresh(
        isCurrent: @escaping StatusRefreshCoordinator.IsCurrent
    ) async {
        let scope = pendingStatusRefreshScope ?? .full
        pendingStatusRefreshScope = nil
        // Health ticks run every 10 s over HTTP; skip the extra CLI process there
        // and reuse the Codex status from the last status or full refresh.
        if scope != .health {
            do {
                _ = try await refreshCodexIntegrationStatus()
            } catch {
                appendDiagnosticLog(
                    "Codex integration status refresh failed: \(localizedErrorDescription(error))"
                )
            }
        }
        if scope == .health {
            do {
                let health = try await checkGatewayHealth()
                guard isCurrent() else { return }
                applyHealthyGatewaySnapshot(health)
                if health.providerReadiness == "degraded" {
                    let status = try await runGateway(["status", "--json"])
                    guard isCurrent() else { return }
                    applyGatewayStatus(status)
                }
            } catch {
                do {
                    let status = try await runGateway(["status", "--json"])
                    guard isCurrent() else { return }
                    applyGatewayStatus(status)
                } catch {
                    guard isCurrent() else { return }
                    _ = applyGatewayStatusFailure(error)
                }
            }
            return
        }

        do {
            let status = try await runGateway(["status", "--json"])
            guard isCurrent() else { return }
            applyGatewayStatus(status)
        } catch {
            guard isCurrent() else { return }
            if applyGatewayStatusFailure(error) {
                return
            }
        }
        guard scope == .full || quotaRefreshPolicy.isDue() else { return }
        quotaRefreshPolicy.markAttempt()
        // Quota pages can involve several remote providers and browser-backed
        // dashboards. Start both subprocesses together so a slow quota probe
        // cannot hide the local token history on a fresh app launch.
        let providersTask = Task { try await runGateway(["providers", "list", "--json"]) }
        let quotaTask = Task { try await runGateway(["quota", "--json"]) }
        let usageRange = providerUsageWindowController?.model.selectedRange ?? .all
        let usageTask = Task { try await runGateway(usageRange.commandArguments) }
        do {
            let providerList = try decodeProviderList(await providersTask.value)
            guard isCurrent() else { return }
            let dashboardProviders = providerList.providers.map {
                ProviderDashboardProvider(
                    id: $0.id,
                    displayName: providerDisplayNameForGatewayState(
                        $0.displayName,
                        isGatewayRunning: isRunning,
                        codexStatus: codexIntegrationStatus
                    ),
                    isEnabled: $0.enabled,
                    websiteURL: $0.websiteURL
                )
            }
            await MainActor.run {
                providerUsageWindowController?.updateConfiguredProviders(dashboardProviders)
                providerUsageDashboardView?.updateConfiguredProviders(dashboardProviders)
            }
            Task { [weak self] in
                guard let self else { return }
                for provider in dashboardProviders {
                    guard let websiteURL = provider.websiteURL else { continue }
                    do {
                        if try await refreshProviderLogoIfNeeded(
                            providerID: provider.id,
                            websiteURL: websiteURL
                        ) {
                            await MainActor.run {
                                self.providerUsageWindowController?.refreshProviderIcons()
                                self.providerUsageDashboardView?.refreshProviderIcons()
                            }
                        }
                    } catch {
                        appendAppDiagnosticLog(
                            "provider icon refresh failed for \(provider.id): \(diagnosticErrorDescription(error))",
                            directory: self.stateDir()
                        )
                    }
                }
            }
        } catch {
            guard isCurrent() else { return }
            updateQuotaStatus(
                title: "Provider 列表：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
        do {
            let usage = try await usageTask.value
            guard isCurrent() else { return }
            updateProviderTokenUsageStatus(try parseProviderTokenUsage(usage))
        } catch {
            guard isCurrent() else { return }
            updateTokenUsageStatus(
                title: "Token 使用：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
        do {
            let quota = try await quotaTask.value
            guard isCurrent() else { return }
            saveProviderQuotaCache(quota)
            updateProviderQuotaStatus(try parseProviderQuotaUsage(quota))
        } catch {
            guard isCurrent() else { return }
            updateQuotaStatus(
                title: "Provider 额度：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
    }

    private func checkGatewayHealth() async throws -> GatewayHealthResponse {
        guard
            let serviceEndpoint,
            var healthURL = URL(string: serviceEndpoint)
        else {
            throw GatewayError.command("gateway endpoint is unavailable")
        }
        if healthURL.lastPathComponent == "v1" {
            healthURL.deleteLastPathComponent()
        }
        healthURL.appendPathComponent("healthz")
        var request = URLRequest(url: healthURL)
        request.timeoutInterval = 2
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let httpResponse = response as? HTTPURLResponse,
              (200 ... 299).contains(httpResponse.statusCode)
        else {
            throw GatewayError.command("gateway health check failed")
        }
        let health = try JSONDecoder().decode(GatewayHealthResponse.self, from: data)
        guard health.ok else {
            throw GatewayError.command("gateway health check failed")
        }
        return health
    }

    private func applyHealthyGatewaySnapshot(_ health: GatewayHealthResponse) {
        isRunning = true
        if health.providerReadiness == "degraded" {
            if !serviceStatus.contains("服务商需要处理") {
                serviceStatus = "本地服务运行中 · 服务商需要处理"
            }
            if providerStatusDetail == nil {
                providerStatusDetail = "服务商配置或模型列表需要处理"
            }
        } else if health.providerReadiness == "disabled" {
            providerStatusDetail = nil
            serviceStatus = "本地服务运行中 · 无启用服务商"
        } else {
            providerStatusDetail = nil
            serviceStatus = "本地服务运行中"
        }
        if let reason = echFallbackReason {
            serviceStatus += " · ECH 已回退直连"
            providerStatusDetail = reason
        }
        updateStatusTitle()
        updateActionStates()
    }

    @discardableResult
    private func applyGatewayStatusFailure(_ error: Error) -> Bool {
        isRunning = false
        serviceEndpoint = nil
        if codexIntegrationStatus?.isOfficialMode == true {
            serviceStatus = stoppedGatewayStatusTitle(codexStatus: codexIntegrationStatus)
        } else {
            serviceStatus = "网关状态检查失败"
            providerStatusDetail = localizedErrorDescription(error)
        }
        updateStatusTitle()
        updateActionStates()
        return false
    }

    func updateQuotaStatus(title: String, detail: String?, progress: Double?) {
        providerUsageWindowController?.updateQuotaStatus(title: title, detail: detail)
        providerUsageDashboardView?.updateQuotaStatus(title: title, detail: detail)
    }

    func updateProviderQuotaStatus(_ usages: [ProviderQuotaUsage]) {
        providerUsageWindowController?.updateQuotaUsages(usages)
        providerUsageDashboardView?.updateQuotaUsages(usages)
    }

    func loadCachedProviderQuota() {
        let url = stateDir().appendingPathComponent("quota-cache.json")
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        do {
            let rawJSON = try String(contentsOf: url, encoding: .utf8)
            updateProviderQuotaStatus(try parseProviderQuotaUsage(rawJSON))
        } catch {
            appendDiagnosticLog(
                "Failed to load cached provider quota: \(localizedErrorDescription(error))"
            )
        }
    }

    private func saveProviderQuotaCache(_ rawJSON: String) {
        let url = stateDir().appendingPathComponent("quota-cache.json")
        do {
            try FileManager.default.createDirectory(
                at: stateDir(),
                withIntermediateDirectories: true
            )
            try rawJSON.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            appendDiagnosticLog(
                "Failed to save provider quota cache: \(localizedErrorDescription(error))"
            )
        }
    }

    func updateTokenUsageStatus(title: String, detail: String?, progress: Double?) {
        providerUsageWindowController?.updateTokenStatus(title: title, detail: detail)
    }

    func updateProviderTokenUsageStatus(_ usages: [ProviderTokenUsage]) {
        providerUsageWindowController?.updateTokenUsages(usages)
    }

    @MainActor
    func refreshTokenUsage(for range: TokenUsageRange) async {
        do {
            let usage = try await runGateway(range.commandArguments)
            guard providerUsageWindowController?.model.selectedRange == range else { return }
            updateProviderTokenUsageStatus(try parseProviderTokenUsage(usage))
        } catch {
            guard providerUsageWindowController?.model.selectedRange == range else { return }
            updateTokenUsageStatus(
                title: "Token 使用：不可用",
                detail: localizedErrorDescription(error),
                progress: nil
            )
        }
    }

    @MainActor
    func refreshRequestUsage(providerID: String, before: Int64?) async {
        var arguments = [
            "request-usage", "--json", "--limit", "\(requestPageSize)", "--provider", providerID,
        ]
        if let before {
            arguments += ["--before", "\(before)"]
        }
        do {
            let json = try await runGateway(arguments)
            providerUsageWindowController?.appendRequestPage(
                providerID: providerID,
                rows: try parseRequestUsage(json)
            )
        } catch {
            providerUsageWindowController?.updateRequestStatus(
                "请求明细：不可用（\(localizedErrorDescription(error))）"
            )
        }
    }

    /// Activity failures replace the activity sections with the error; the
    /// rest of the page keeps working.
    @MainActor
    func refreshUsageActivity(providerID: String, range: TokenUsageRange) async {
        var arguments = ["usage-activity", "--json", "--provider", providerID]
        if let days = range.days {
            arguments += ["--days", "\(days)"]
        }
        do {
            let json = try await runGateway(arguments)
            providerUsageWindowController?.updateActivity(
                providerID: providerID,
                range: range,
                activity: try parseUsageActivity(json)
            )
        } catch {
            providerUsageWindowController?.updateActivityError(
                providerID: providerID,
                "用量活动：不可用（\(localizedErrorDescription(error))）"
            )
        }
    }
}
