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
        isRunning = status?.contains("gateway: running") == true
        let providerIssues = providerIssueDetails(fromGatewayStatus: status)
        providerStatusDetail = providerIssues.isEmpty
            ? nil
            : providerIssues.joined(separator: "；")
        let providerReadiness = status?
            .split(separator: "\n")
            .first(where: { $0.hasPrefix("provider-readiness: ") })
            .map { String($0.dropFirst("provider-readiness: ".count)) }
        serviceEndpoint = status?
            .split(separator: "\n")
            .first(where: { $0.hasPrefix("endpoint: ") })
            .map { String($0.dropFirst("endpoint: ".count)) }
        if isRunning, providerReadiness == "degraded" {
            serviceStatus = "本地网关运行中 · Provider 降级"
        } else if isRunning, providerReadiness == "disabled" {
            serviceStatus = "本地网关运行中 · 无启用 Provider"
        } else {
            serviceStatus = isRunning ? "本地网关运行中" : "本地网关已停止"
        }
        updateStatusTitle()
        updateActionStates()
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
        if scope == .health {
            do {
                let health = try await checkGatewayHealth()
                guard isCurrent() else { return }
                applyHealthyGatewaySnapshot(health)
            } catch {
                do {
                    let status = try await runGateway(["status"])
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
            let status = try await runGateway(["status"])
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
        let usageTask = Task { try await runGateway(["usage", "--json"]) }
        do {
            let providerList = try decodeProviderList(await providersTask.value)
            guard isCurrent() else { return }
            let dashboardProviders = providerList.providers.map {
                ProviderDashboardProvider(
                    id: $0.id,
                    displayName: $0.displayName,
                    isEnabled: $0.enabled,
                    websiteURL: $0.websiteURL
                )
            }
            await MainActor.run {
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
            serviceStatus = "本地网关运行中 · Provider 降级"
            if providerStatusDetail == nil {
                providerStatusDetail = "Provider 配置或模型缓存需要处理"
            }
        } else if health.providerReadiness == "disabled" {
            providerStatusDetail = nil
            serviceStatus = "本地网关运行中 · 无启用 Provider"
        } else {
            providerStatusDetail = nil
            serviceStatus = "本地网关运行中"
        }
        updateStatusTitle()
        updateActionStates()
    }

    @discardableResult
    private func applyGatewayStatusFailure(_ error: Error) -> Bool {
        let message = String(describing: error)
        let missingConfiguration = isMissingGatewayConfiguration(error)
        isRunning = false
        serviceEndpoint = nil
        if missingConfiguration {
            serviceStatus = "等待配置上游 API"
        } else if message.contains("gateway not running") {
            serviceStatus = "本地网关已停止"
        } else {
            serviceStatus = "网关状态检查失败"
        }
        updateStatusTitle()
        updateActionStates()
        if missingConfiguration {
            updateQuotaStatus(title: "额度：等待配置", detail: nil, progress: nil)
        }
        return missingConfiguration
    }

    func updateQuotaStatus(title: String, detail: String?, progress: Double?) {
        providerUsageDashboardView?.updateQuotaStatus(title: title, detail: detail)
    }

    func updateProviderQuotaStatus(_ usages: [ProviderQuotaUsage]) {
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
        providerUsageDashboardView?.updateTokenStatus(title: title, detail: detail)
    }

    func updateProviderTokenUsageStatus(_ usages: [ProviderTokenUsage]) {
        providerUsageDashboardView?.updateTokenUsages(usages)
    }
}
