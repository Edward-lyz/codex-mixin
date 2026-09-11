import Cocoa

enum ProviderIssueAction: Equatable {
    case connection
    case models
    case toggle
}

struct ProviderIssuePresentation: Equatable {
    let title: String
    let detail: String
    let actionTitle: String?
    let action: ProviderIssueAction
}

func providerIssuePresentations(for provider: ProviderView) -> [ProviderIssuePresentation] {
    guard provider.kind == .configured else { return [] }
    guard provider.enabled else {
        return [ProviderIssuePresentation(
            title: "已停用",
            detail: "该服务商的自定义模型不会承接请求。",
            actionTitle: "启用",
            action: .toggle
        )]
    }

    let issues = Set(provider.readinessIssues)
    var presentations: [ProviderIssuePresentation] = []
    if issues.contains("credentials_missing") {
        presentations.append(ProviderIssuePresentation(
            title: "尚未配置密钥",
            detail: "该服务商暂不能使用。",
            actionTitle: "填写密钥",
            action: .connection
        ))
    }
    if issues.contains("no_routable_models") {
        if provider.selectedModels.isEmpty, !provider.cachedModels.isEmpty {
            presentations.append(ProviderIssuePresentation(
                title: "还没有加入模型",
                detail: "从下方选择模型后应用更改。",
                actionTitle: "选择模型",
                action: .models
            ))
        } else if provider.cachedModels.isEmpty {
            presentations.append(ProviderIssuePresentation(
                title: "正在后台获取模型列表",
                detail: "网关会自动同步，获取后即可选择模型。",
                actionTitle: nil,
                action: .models
            ))
        } else {
            presentations.append(ProviderIssuePresentation(
                title: "当前已选模型暂不可用",
                detail: "后台会继续更新模型列表；也可以重新选择模型。",
                actionTitle: "重新选择",
                action: .models
            ))
        }
    }
    if issues.contains("selected_models_unavailable"),
       !provider.unavailableSelectedModels.isEmpty,
       provider.routableModelCount > 0 {
        presentations.append(ProviderIssuePresentation(
            title: "\(provider.unavailableSelectedModels.count) 个已选模型不在当前列表",
            detail: "其余 \(provider.routableModelCount) 个模型仍可使用。",
            actionTitle: "查看模型",
            action: .models
        ))
    }
    if issues.contains("model_refresh_failed") {
        let cachedList = provider.cachedModels.isEmpty
            ? "没有可用的历史模型列表。"
            : "当前显示上次成功获取的模型列表。"
        let error = provider.lastModelRefreshError?
            .split(whereSeparator: \.isNewline)
            .joined(separator: " ")
        presentations.append(ProviderIssuePresentation(
            title: "模型列表更新失败",
            detail: [cachedList, error].compactMap { value in
                guard let value, !value.isEmpty else { return nil }
                return value
            }.joined(separator: " ") + " 网关将在后台自动重试。",
            actionTitle: nil,
            action: .models
        ))
    }
    if presentations.isEmpty, provider.readiness == "degraded" {
        presentations.append(ProviderIssuePresentation(
            title: "配置需要处理",
            detail: "请核对连接设置和模型选择。",
            actionTitle: "查看设置",
            action: .connection
        ))
    }
    return presentations
}

func providerPrimaryStatus(_ provider: ProviderView) -> String {
    if provider.kind == .official {
        return "官方账号 · 已加入 \(provider.selectedModels.count) 个模型"
    }
    if let issue = providerIssuePresentations(for: provider).first {
        return issue.title
    }
    return "已加入 \(provider.selectedModels.count) 个模型 · 配置就绪"
}

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
        return providersEmpty ? "等待新增服务商" : "请选择服务商"
    }
    if provider.kind == .official {
        return "OpenAI 官方 OAuth 登录 · 已加入 \(provider.selectedModels.count) 个模型 · 跟随 Codex Mixin 安装模式 · 只读"
    }
    let refresh: String
    if let milliseconds = provider.modelsRefreshedAtMilliseconds {
        refresh = "模型缓存更新于 \(formatProviderTimestamp(milliseconds))"
    } else {
        refresh = "尚未在线刷新模型"
    }
    var details = [
        providerPrimaryStatus(provider),
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
        return "需要处理"
    case "disabled":
        return "停用"
    default:
        return readiness
    }
}
