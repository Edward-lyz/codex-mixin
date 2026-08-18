import Cocoa

func auxiliaryModelDefaultTooltip(for codexInstallMode: ManagedCodexInstallMode?) -> String {
    switch codexInstallMode {
    case .customOnly:
        return AppLocalization.string("providerSettings.onlyOneAuxiliaryModelProviderCanBe")
    case .codexOAuthProxy:
        return AppLocalization.string("providerSettings.onlyOneAuxiliaryModelProviderCanBe2")
    case nil:
        return AppLocalization.string("providerSettings.onlyOneAuxiliaryModelProviderCanBe3")
    }
}

func auxiliaryModelTooltip(for provider: ProviderView, codexInstallMode: ManagedCodexInstallMode?) -> String {
    let capability: String
    switch (codexInstallMode, provider.auxiliaryModelSupport) {
    case (.customOnly, .none):
        capability = AppLocalization.string("providerSettings.thisProviderSupportsNeitherAutoReviewNor")
    case (.customOnly, .autoReviewOnly):
        capability = AppLocalization.string("providerSettings.thisProviderSupportsAutoReviewOnlyVoice")
    case (.customOnly, .voiceOnly):
        capability = AppLocalization.string("providerSettings.thisProviderSupportsVoiceOnlyAutoReview")
    case (.codexOAuthProxy, .none):
        capability = AppLocalization.string("providerSettings.thisProviderOffersNeitherAutoReviewNor")
    case (.codexOAuthProxy, .autoReviewOnly):
        capability = AppLocalization.string("providerSettings.thisProviderOffersAutoReviewOnlyVoice")
    case (.codexOAuthProxy, .voiceOnly):
        capability = AppLocalization.string("providerSettings.thisProviderOffersVoiceOnlyAutoReview")
    case (_, .none):
        capability = AppLocalization.string("providerSettings.theCurrentModelCacheContainsNoAuto")
    case (_, .autoReviewOnly):
        capability = AppLocalization.string("providerSettings.thisProviderSupportsAutoReviewOnly")
    case (_, .voiceOnly):
        capability = AppLocalization.string("providerSettings.thisProviderSupportsVoiceOnly")
    case (_, .autoReviewAndVoice):
        capability = AppLocalization.string("providerSettings.thisProviderSupportsBothAutoReviewAnd")
    }
    return "\(capability)\n\n\(auxiliaryModelDefaultTooltip(for: codexInstallMode))"
}

func auxiliaryModelStatus(for provider: ProviderView, codexInstallMode: ManagedCodexInstallMode?) -> String? {
    switch (codexInstallMode, provider.auxiliaryModelSupport) {
    case (.customOnly, .none):
        return "辅助模型不可用：自动审查和语音均不支持"
    case (_, .autoReviewOnly):
        return "辅助模型：仅支持自动审查"
    case (_, .voiceOnly):
        return "辅助模型：仅支持语音"
    case (.codexOAuthProxy, .none):
        return "辅助模型：使用 OAuth 默认路由"
    case (_, .none), (_, .autoReviewAndVoice):
        return nil
    }
}

func selectedProviderStatus(provider: ProviderView?, providersEmpty: Bool, codexInstallMode: ManagedCodexInstallMode?) -> String {
    guard let provider else {
        return providersEmpty ? "等待新增 Provider" : "请选择 Provider"
    }
    if provider.kind == .official {
        return "OpenAI 官方 OAuth 登录 · \(readinessLabel(provider.readiness)) · 跟随 Codex Mixin 安装模式 · 只读"
    }
    let refresh: String
    if let milliseconds = provider.modelsRefreshedAtMilliseconds {
        refresh = "模型缓存更新于 \(formatProviderTimestamp(milliseconds))"
    } else {
        refresh = "尚未在线刷新模型"
    }
    var details = [
        "\(provider.routableModelCount) 个模型可路由",
        "\(provider.newModels.count) 个新增",
        "\(provider.unavailableSelectedModels.count) 个不可用",
        refresh,
    ]
    if let auxiliaryStatus = auxiliaryModelStatus(for: provider, codexInstallMode: codexInstallMode) {
        details.insert(auxiliaryStatus, at: 0)
    }
    if provider.lastModelRefreshError != nil {
        details.append("上次刷新失败")
    }
    return details.joined(separator: " · ")
}

func formatProviderTimestamp(_ milliseconds: UInt64) -> String {
    let date = Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1_000)
    let formatter = DateFormatter()
    formatter.dateStyle = .short
    formatter.timeStyle = .short
    return formatter.string(from: date)
}

func readinessLabel(_ readiness: String) -> String {
    switch readiness {
    case "healthy":
        return "正常"
    case "degraded":
        return "降级"
    case "disabled":
        return "停用"
    default:
        return readiness
    }
}
