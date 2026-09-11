import Cocoa
import SwiftUI

struct ProviderSettingsRow: Identifiable {
    let provider: ProviderView

    var id: String { provider.id }
}

private struct ProviderConnectionDraft {
    let displayName: String
    let baseURL: String
    let protocolID: String
    let websiteURL: String
    let imageGenerationPath: String
    let apiKey: String
    let awsAccessKeyID: String
    let awsSecretAccessKey: String
    let awsSessionToken: String
    let awsRegion: String
    let clearAwsSessionToken: Bool
    let quotaUsername: String
    let quotaWorkspaceID: String
    let quotaAuthCookie: String
    let auxiliaryModelUpstream: Bool
    let autoReviewModel: String
    let baiduAuthBridge: BaiduAuthBridgeMode
    let baiduCodeReport: Bool
}

@MainActor
final class ProviderSettingsModel: ObservableObject {
    @Published var providers: [ProviderView] = []
    @Published var selectedProviderID: String?
    @Published var status = "正在读取供应商…"
    @Published var isBusy = false
    @Published var banner: ProviderBannerState?
    @Published var codexInstallMode: ManagedCodexInstallMode?
    @Published var displayName = ""
    @Published var baseURL = ""
    @Published var protocolID = "open_ai_responses"
    @Published var websiteURL = ""
    @Published var imageGenerationPath = ""
    @Published var apiKey = ""
    @Published var awsAccessKeyID = ""
    @Published var awsSecretAccessKey = ""
    @Published var awsSessionToken = ""
    @Published var awsRegion = "us-east-1"
    @Published var clearAwsSessionToken = false
    @Published var quotaUsername = ""
    @Published var quotaWorkspaceID = ""
    @Published var quotaAuthCookie = ""
    @Published var auxiliaryModelUpstream = false
    @Published var autoReviewModel = ""
    @Published var baiduAuthBridge = BaiduAuthBridgeMode.disabled
    @Published var baiduCodeReport = false
    @Published var applyRetryProviderID: String?
    @Published var externalChangeProviderIDs: Set<String> = []

    private var drafts: [String: ProviderConnectionDraft] = [:]

    var selectedProvider: ProviderView? {
        providers.first { $0.id == selectedProviderID }
    }

    var canAddProvider: Bool { !isBusy }
    var canModifySelectedProvider: Bool {
        !isBusy && selectedProvider?.kind == .configured
    }

    var connectionDirty: Bool {
        guard let provider = selectedProvider, provider.kind == .configured else { return false }
        return draftIsDirty(currentDraft(), for: provider)
    }

    var hasConnectionDrafts: Bool {
        if connectionDirty { return true }
        return drafts.contains { providerID, draft in
            guard providerID != selectedProviderID,
                  let provider = providers.first(where: { $0.id == providerID })
            else {
                return false
            }
            return draftIsDirty(draft, for: provider)
        }
    }

    var applyRetryRequired: Bool {
        applyRetryProviderID == selectedProviderID
    }

    var hasPendingApplyRetry: Bool {
        applyRetryProviderID != nil
    }

    var selectedProviderHasExternalChange: Bool {
        selectedProviderID.map(externalChangeProviderIDs.contains) == true
    }

    private func draftIsDirty(_ draft: ProviderConnectionDraft, for provider: ProviderView) -> Bool {
        if !draft.apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return true }
        if draft.imageGenerationPath != (provider.imageGenerationPath ?? "") { return true }
        if draft.auxiliaryModelUpstream != provider.auxiliaryModelUpstream { return true }
        if draft.autoReviewModel != (provider.autoReviewModel ?? "") { return true }
        if provider.presetID == "custom" {
            return draft.displayName != provider.displayName
                || draft.baseURL != provider.baseURL
                || draft.protocolID != provider.protocolID
                || draft.websiteURL != (provider.websiteURL ?? "")
        }
        if provider.presetID == "aws-bedrock" {
            return draft.awsRegion != (provider.awsRegion ?? "us-east-1")
                || !draft.awsAccessKeyID.isEmpty
                || !draft.awsSecretAccessKey.isEmpty
                || !draft.awsSessionToken.isEmpty
                || draft.clearAwsSessionToken
        }
        if provider.presetID == "baidu-oneapi" {
            return draft.quotaUsername != (provider.quotaUsername ?? "")
                || draft.baiduAuthBridge != (provider.effectiveBaiduAuthBridge ?? .disabled)
                || draft.baiduCodeReport != (provider.baiduCodeReport == true)
        }
        if requiresOpenCodeGoQuotaCredentials(provider.presetID ?? "") {
            return draft.quotaWorkspaceID != (provider.quotaWorkspaceID ?? "")
                || !draft.quotaAuthCookie.isEmpty
        }
        return false
    }

    func rows() -> [ProviderSettingsRow] {
        providers.map(ProviderSettingsRow.init(provider:))
    }

    func selectProvider(_ providerID: String?) {
        saveCurrentDraft()
        selectedProviderID = providerID
        loadSelectedProviderState()
    }

    func discardSelectedDraft() {
        guard let providerID = selectedProviderID else { return }
        drafts.removeValue(forKey: providerID)
        externalChangeProviderIDs.remove(providerID)
        loadSelectedProviderState()
    }

    func acknowledgeSelectedExternalChange() {
        guard let providerID = selectedProviderID else { return }
        externalChangeProviderIDs.remove(providerID)
    }

    func discardAllDrafts() {
        drafts.removeAll()
        applyRetryProviderID = nil
        externalChangeProviderIDs.removeAll()
        loadSelectedProviderState()
    }

    func clearSensitiveDrafts() {
        saveCurrentDraft()
        drafts = drafts.mapValues { draft in
            ProviderConnectionDraft(
                displayName: draft.displayName,
                baseURL: draft.baseURL,
                protocolID: draft.protocolID,
                websiteURL: draft.websiteURL,
                imageGenerationPath: draft.imageGenerationPath,
                apiKey: "",
                awsAccessKeyID: "",
                awsSecretAccessKey: "",
                awsSessionToken: "",
                awsRegion: draft.awsRegion,
                clearAwsSessionToken: draft.clearAwsSessionToken,
                quotaUsername: draft.quotaUsername,
                quotaWorkspaceID: draft.quotaWorkspaceID,
                quotaAuthCookie: "",
                auxiliaryModelUpstream: draft.auxiliaryModelUpstream,
                autoReviewModel: draft.autoReviewModel,
                baiduAuthBridge: draft.baiduAuthBridge,
                baiduCodeReport: draft.baiduCodeReport
            )
        }
        apiKey = ""
        awsAccessKeyID = ""
        awsSecretAccessKey = ""
        awsSessionToken = ""
        quotaAuthCookie = ""
    }

    func replaceProviders(
        _ providers: [ProviderView],
        selecting providerID: String?,
        preserveDrafts: Bool
    ) {
        if preserveDrafts {
            saveCurrentDraft()
            for (providerID, draft) in drafts {
                guard let previous = self.providers.first(where: { $0.id == providerID }),
                      let updated = providers.first(where: { $0.id == providerID }),
                      draftIsDirty(draft, for: previous),
                      providerFingerprint(previous) != providerFingerprint(updated)
                else {
                    continue
                }
                externalChangeProviderIDs.insert(providerID)
            }
        } else if let providerID {
            drafts.removeValue(forKey: providerID)
            externalChangeProviderIDs.remove(providerID)
        }
        self.providers = providers
        selectedProviderID = providerID
        loadSelectedProviderState()
    }

    private func loadSelectedProviderState() {
        guard let provider = selectedProvider else {
            status = providers.isEmpty ? "等待新增服务商" : "请选择服务商"
            return
        }
        apply(drafts[provider.id] ?? ProviderConnectionDraft(
            displayName: provider.displayName,
            baseURL: provider.baseURL,
            protocolID: provider.protocolID,
            websiteURL: provider.websiteURL ?? "",
            imageGenerationPath: provider.imageGenerationPath ?? "",
            apiKey: "",
            awsAccessKeyID: "",
            awsSecretAccessKey: "",
            awsSessionToken: "",
            awsRegion: provider.awsRegion ?? "us-east-1",
            clearAwsSessionToken: false,
            quotaUsername: provider.quotaUsername ?? "",
            quotaWorkspaceID: provider.quotaWorkspaceID ?? "",
            quotaAuthCookie: "",
            auxiliaryModelUpstream: provider.auxiliaryModelUpstream,
            autoReviewModel: provider.autoReviewModel ?? "",
            baiduAuthBridge: provider.effectiveBaiduAuthBridge ?? .disabled,
            baiduCodeReport: provider.baiduCodeReport == true
        ))
        status = selectedProviderStatus(
            provider: provider,
            providersEmpty: providers.isEmpty,
            codexInstallMode: codexInstallMode
        )
    }

    private func saveCurrentDraft() {
        guard let providerID = selectedProviderID else { return }
        drafts[providerID] = currentDraft()
    }

    private func currentDraft() -> ProviderConnectionDraft {
        ProviderConnectionDraft(
            displayName: displayName,
            baseURL: baseURL,
            protocolID: protocolID,
            websiteURL: websiteURL,
            imageGenerationPath: imageGenerationPath,
            apiKey: apiKey,
            awsAccessKeyID: awsAccessKeyID,
            awsSecretAccessKey: awsSecretAccessKey,
            awsSessionToken: awsSessionToken,
            awsRegion: awsRegion,
            clearAwsSessionToken: clearAwsSessionToken,
            quotaUsername: quotaUsername,
            quotaWorkspaceID: quotaWorkspaceID,
            quotaAuthCookie: quotaAuthCookie,
            auxiliaryModelUpstream: auxiliaryModelUpstream,
            autoReviewModel: autoReviewModel,
            baiduAuthBridge: baiduAuthBridge,
            baiduCodeReport: baiduCodeReport
        )
    }

    private func apply(_ draft: ProviderConnectionDraft) {
        displayName = draft.displayName
        baseURL = draft.baseURL
        protocolID = draft.protocolID
        websiteURL = draft.websiteURL
        imageGenerationPath = draft.imageGenerationPath
        apiKey = draft.apiKey
        awsAccessKeyID = draft.awsAccessKeyID
        awsSecretAccessKey = draft.awsSecretAccessKey
        awsSessionToken = draft.awsSessionToken
        awsRegion = draft.awsRegion
        clearAwsSessionToken = draft.clearAwsSessionToken
        quotaUsername = draft.quotaUsername
        quotaWorkspaceID = draft.quotaWorkspaceID
        quotaAuthCookie = draft.quotaAuthCookie
        auxiliaryModelUpstream = draft.auxiliaryModelUpstream
        autoReviewModel = draft.autoReviewModel
        baiduAuthBridge = draft.baiduAuthBridge
        baiduCodeReport = draft.baiduCodeReport
    }

    private func providerFingerprint(_ provider: ProviderView) -> [String] {
        [
            provider.displayName,
            provider.baseURL,
            provider.protocolID,
            provider.websiteURL ?? "",
            provider.imageGenerationPath ?? "",
            String(provider.auxiliaryModelUpstream),
            provider.autoReviewModel ?? "",
            String(provider.apiKeyConfigured),
            String(provider.awsSigV4Configured == true),
            provider.awsRegion ?? "",
            String(provider.awsSessionTokenConfigured == true),
            provider.quotaUsername ?? "",
            provider.quotaWorkspaceID ?? "",
            String(provider.quotaAuthCookieConfigured == true),
            provider.effectiveBaiduAuthBridge?.rawValue ?? "",
            String(provider.baiduCodeReport == true),
        ]
    }
}

struct ProviderBannerState: Equatable {
    let text: String
    let isError: Bool
}

struct ProviderSettingsRootView: View {
    @ObservedObject var model: ProviderSettingsModel
    @ObservedObject var benchmarkModel: ModelBenchmarkModel
    let onAdd: () -> Void
    let onRemove: () -> Void
    let onToggle: () -> Void
    let onTest: () -> Void
    let onApplyChanges: () -> Void
    let onClearKey: () -> Void
    let onClearQuotaCredentials: () -> Void
    let onMove: (IndexSet, Int) -> Void

    var body: some View {
        NavigationSplitView {
            VStack(spacing: 0) {
                HStack(spacing: 6) {
                    Text("服务商")
                        .font(.headline)
                    Spacer()

                    Button(action: onAdd) {
                        Image(systemName: "plus")
                            .frame(width: 22, height: 22)
                    }
                    .buttonStyle(.borderless)
                    .controlSize(.small)
                    .keyboardShortcut("n", modifiers: .command)
                    .help("新增服务商（⌘N）")
                    .accessibilityLabel("新增服务商")
                    .disabled(!model.canAddProvider || benchmarkModel.isBusy)

                    Button(action: onRemove) {
                        Image(systemName: "minus")
                            .frame(width: 22, height: 22)
                    }
                    .buttonStyle(.borderless)
                    .controlSize(.small)
                    .help("删除服务商")
                    .accessibilityLabel("删除服务商")
                    .disabled(!model.canModifySelectedProvider || benchmarkModel.isBusy)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)

                List(selection: Binding(
                    get: { model.selectedProviderID },
                    set: {
                        model.selectProvider($0)
                        benchmarkModel.selectProvider($0)
                    }
                )) {
                    if model.rows().isEmpty {
                        Label("还没有服务商", systemImage: "plus.circle")
                            .foregroundStyle(.secondary)
                            .padding(.vertical, 6)
                    } else {
                        ForEach(model.rows()) { row in
                            ProviderSidebarRow(provider: row.provider)
                                .tag(row.id)
                                .moveDisabled(row.provider.kind == .official)
                        }
                        .onMove(perform: onMove)
                    }
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)
                .background(Color.clear)

            }
            .background(Color(nsColor: .windowBackgroundColor))
            .clipShape(Rectangle())
            .navigationSplitViewColumnWidth(
                min: providerSidebarMinimumWidth,
                ideal: providerSidebarIdealWidth,
                max: providerSidebarMaximumWidth
            )
        } detail: {
            VStack(alignment: .leading, spacing: 0) {
                if let banner = model.banner {
                    HStack {
                        Label {
                            Text(banner.text)
                                .lineLimit(2)
                        } icon: {
                            Image(systemName: banner.isError ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                        }
                        Spacer()
                    }
                    .font(.footnote.weight(.medium))
                    .foregroundStyle(banner.isError ? Color(nsColor: .systemRed) : Color(nsColor: .systemGreen))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .background(
                        (banner.isError ? Color(nsColor: .systemRed) : Color(nsColor: .systemGreen)).opacity(0.10),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous)
                    )
                    .padding(.horizontal, 24)
                    .padding(.top, 14)
                }

                if let provider = model.selectedProvider {
                    ProviderDetailForm(
                        provider: provider,
                        codexInstallMode: model.codexInstallMode,
                        isBusy: model.isBusy,
                        formState: model,
                        benchmarkModel: benchmarkModel,
                        onToggle: onToggle,
                        onTest: onTest,
                        onApplyChanges: onApplyChanges,
                        onClearKey: onClearKey,
                        onClearQuotaCredentials: onClearQuotaCredentials
                    )
                    .id(provider.id)
                } else {
                    ProviderEmptyState(
                        isEmpty: model.providers.isEmpty,
                        onAdd: onAdd
                    )
                }
            }
            .background(Color(nsColor: .windowBackgroundColor))
        }
        .navigationTitle("模型与服务")
    }

}

private struct ProviderEmptyState: View {
    let isEmpty: Bool
    let onAdd: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: isEmpty ? "server.rack" : "cursorarrow.click.2")
                .font(.system(size: 28, weight: .medium))
                .foregroundStyle(.tertiary)
            Text(isEmpty ? "还没有服务商" : "选择一个服务商")
                .font(.title2.weight(.semibold))
            Text(
                isEmpty
                    ? "添加上游 API 后，Codex Mixin 才能为网关提供模型。"
                    : "从左侧选择一个服务商，查看连接设置、模型和测速结果。"
            )
            .font(.body)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            if isEmpty {
                Button("新增服务商", action: onAdd)
                    .liquidGlassProminentButton()
                    .controlSize(.large)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .frame(maxWidth: 440)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(32)
    }
}

private struct ProviderSidebarRow: View {
    let provider: ProviderView

    var body: some View {
        HStack(spacing: 10) {
            ProviderLogo(provider: provider, size: 28)

            VStack(alignment: .leading, spacing: 3) {
                Text(provider.displayName)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                HStack(spacing: 5) {
                    Circle()
                        .fill(providerStatusColor(provider))
                        .frame(width: 6, height: 6)
                    Text(sidebarMetadata)
                        .font(.caption)
                        .lineLimit(1)
                }
                .foregroundStyle(.secondary)
            }

            Spacer(minLength: 4)

            if provider.kind == .configured {
                Image(systemName: "line.3.horizontal")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.tertiary)
                    .help("拖动以排序")
                    .accessibilityLabel("可拖动排序")
            }
        }
        .padding(.vertical, 4)
    }

    private var sidebarMetadata: String {
        "\(readinessLabel(provider.readiness)) · \(provider.selectedModels.count)/\(provider.cachedModels.count) 个模型"
    }
}

private enum ProviderDetailSection: String, CaseIterable, Identifiable {
    case models
    case connection

    var id: Self { self }

    var title: String {
        switch self {
        case .models: return "模型"
        case .connection: return "连接设置"
        }
    }
}

private struct ProviderDetailForm: View {
    let provider: ProviderView
    let codexInstallMode: ManagedCodexInstallMode?
    let isBusy: Bool
    @ObservedObject var formState: ProviderSettingsModel
    @ObservedObject var benchmarkModel: ModelBenchmarkModel
    let onToggle: () -> Void
    let onTest: () -> Void
    let onApplyChanges: () -> Void
    let onClearKey: () -> Void
    let onClearQuotaCredentials: () -> Void
    @State private var selectedSection = ProviderDetailSection.models
    @State private var showsAdvancedOptions = false

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                ProviderDetailHeader(provider: provider)
                Spacer()
            }
            .overlay {
                Picker("详情页面", selection: $selectedSection) {
                    ForEach(ProviderDetailSection.allCases) { section in
                        Text(detailSectionTitle(section)).tag(section)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 360)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)

            Divider()

            issueArea
            conflictArea

            Group {
                switch selectedSection {
                case .models:
                    ModelBenchmarkRootView(
                        model: benchmarkModel,
                        embedded: true,
                        onApplyChanges: onApplyChanges,
                        canApplyChanges: canApplyChanges,
                        applySummary: applySummary,
                        hasExternalDraft: formState.connectionDirty || formState.applyRetryRequired,
                        isExternallyBusy: isBusy
                    )
                case .connection:
                    connectionSettingsPage
                }
            }
            .id(selectedSection)
            .transition(.opacity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .animation(.easeInOut(duration: 0.18), value: selectedSection)
    }

    private func detailSectionTitle(_ section: ProviderDetailSection) -> String {
        let hasChanges = section == .models
            ? benchmarkModel.selectedProviderDirty
            : formState.connectionDirty || formState.applyRetryRequired
        return section.title + (hasChanges ? " •" : "")
    }

    private var connectionSettingsPage: some View {
        VStack(spacing: 0) {
            Form {

                Section {
                    LabeledContent("服务商 ID") {
                        Text(provider.id)
                            .font(.body.monospaced())
                            .textSelection(.enabled)
                    }
                    if provider.kind == .official {
                        Label("此服务商由 Codex 官方 OAuth 登录管理，不能在这里修改连接参数。", systemImage: "lock.fill")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    } else {
                        if isCustom {
                            TextField("站点名称", text: $formState.displayName)
                            TextField(
                                AppLocalization.string("settings.apiURL"),
                                text: $formState.baseURL,
                                prompt: Text(AppLocalization.string("settings.apiURLPrompt"))
                            )
                            Text(AppLocalization.string("settings.apiURLHint"))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Picker("API 端点", selection: $formState.protocolID) {
                                Text("Responses").tag("open_ai_responses")
                                Text("Messages").tag("anthropic_messages")
                                Text("Chat Completions").tag("open_ai_chat")
                            }
                            TextField("官网地址", text: $formState.websiteURL)
                        } else if isAWSBedrock {
                            TextField("AWS Region", text: $formState.awsRegion)
                        }
                        if isAWSBedrock {
                            SecureField(awsAccessKeyPrompt, text: $formState.awsAccessKeyID)
                            SecureField(awsSecretKeyPrompt, text: $formState.awsSecretAccessKey)
                            HStack(spacing: 8) {
                                SecureField(awsSessionTokenPrompt, text: $formState.awsSessionToken)
                                if provider.awsSessionTokenConfigured == true {
                                    Button("清除 Session Token") {
                                        formState.awsSessionToken = ""
                                        formState.clearAwsSessionToken = true
                                    }
                                    .disabled(operationsBusy || formState.clearAwsSessionToken)
                                }
                            }
                            if provider.awsSigV4Configured == true {
                                Button("清除 AWS 凭据", action: onClearKey)
                                    .disabled(operationsBusy)
                            }
                        } else {
                            HStack(spacing: 8) {
                                SecureField(apiKeyPrompt, text: $formState.apiKey)
                                if apiKeyConfigured {
                                    Button("清除密钥", action: onClearKey)
                                        .disabled(operationsBusy || !provider.apiKeyConfigured)
                                }
                            }
                        }
                    }
                } header: {
                    Text("连接配置")
                }

                if isBaiduOneAPI {
                    Section {
                        TextField("额度用户名", text: $formState.quotaUsername, prompt: Text("Baidu OneAPI 额度接口必填"))
                    } header: {
                        Text("百度额度")
                    }
                }

                if isOpenCodeGo {
                    Section {
                        TextField("工作区 ID", text: $formState.quotaWorkspaceID, prompt: Text("例如：wrk_abc123"))
                        HStack(spacing: 8) {
                            SecureField("Auth Cookie", text: $formState.quotaAuthCookie, prompt: Text(authCookiePrompt))
                            if quotaCookieConfigured {
                                Button("清除额度凭据", action: onClearQuotaCredentials)
                                    .disabled(operationsBusy || provider.quotaAuthCookieConfigured != true)
                            }
                        }
                    } header: {
                        Text("OpenCode Go")
                    }
                }

                Section {
                    DisclosureGroup("高级选项", isExpanded: $showsAdvancedOptions) {
                        VStack(spacing: 0) {
                            if isBaiduOneAPI {
                                advancedOptionRow(title: "认证桥接") {
                                    Picker("认证桥接", selection: $formState.baiduAuthBridge) {
                                        Text(AppLocalization.string("settings.disabledDefault"))
                                            .tag(BaiduAuthBridgeMode.disabled)
                                        Text("DUCX 核心（loopback）")
                                            .tag(BaiduAuthBridgeMode.ducxLoopback)
                                    }
                                    .labelsHidden()
                                    .frame(width: 220)
                                }
                                advancedOptionDivider
                                advancedOptionRow(title: "上报 AI 代码使用数据") {
                                    Toggle("", isOn: $formState.baiduCodeReport)
                                        .labelsHidden()
                                }
                                if provider.kind == .configured {
                                    advancedOptionDivider
                                }
                            }
                            if provider.kind == .configured {
                                advancedOptionRow(title: "绘图接口路径") {
                                    TextField(
                                        "",
                                        text: $formState.imageGenerationPath,
                                        prompt: Text("/v1/images/generations")
                                    )
                                    .labelsHidden()
                                    .frame(width: 260)
                                }
                                advancedOptionDivider
                            }
                            advancedOptionRow(
                                title: "辅助模型上游",
                                detail: "用于绘图、自动审查和语音等辅助任务"
                            ) {
                                Toggle("", isOn: $formState.auxiliaryModelUpstream)
                                    .labelsHidden()
                                    .disabled(
                                        provider.kind == .official
                                            || !isAuxiliaryModelUpstreamSelectable(
                                                for: provider,
                                                codexInstallMode: codexInstallMode
                                            )
                                    )
                                    .help(auxiliaryModelTooltip(
                                        for: provider,
                                        codexInstallMode: codexInstallMode
                                    ))
                            }
                            if provider.kind == .configured {
                                advancedOptionDivider
                                advancedOptionRow(
                                    title: "自动审查模型",
                                    detail: autoReviewModelDetail(
                                        auxiliaryModelUpstream: formState.auxiliaryModelUpstream
                                    )
                                ) {
                                    Picker("自动审查模型", selection: $formState.autoReviewModel) {
                                        Text("自动").tag("")
                                        ForEach(autoReviewModelChoices(for: provider), id: \.self) {
                                            model in
                                            Text(model).tag(model)
                                        }
                                    }
                                    .labelsHidden()
                                    .frame(width: 260)
                                    .disabled(!formState.auxiliaryModelUpstream)
                                }
                            }
                        }
                        .padding(.top, 10)
                        .padding(.bottom, 4)
                    }
                    .padding(.vertical, 4)
                }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
            .frame(maxWidth: 720, maxHeight: .infinity)
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            ProviderActionBar(
                isBusy: operationsBusy,
                toggleTitle: toggleTitle,
                canModify: formState.canModifySelectedProvider && !benchmarkModel.isBusy,
                hasPendingChanges: formState.connectionDirty
                    || benchmarkModel.selectedProviderDirty
                    || formState.applyRetryRequired,
                canApplyChanges: canApplyChanges,
                applySummary: applySummary,
                onToggle: onToggle,
                onTest: onTest,
                onApplyChanges: onApplyChanges
            )
        }
    }

    private func advancedOptionRow<Control: View>(
        title: String,
        detail: String? = nil,
        @ViewBuilder control: () -> Control
    ) -> some View {
        HStack(alignment: .center, spacing: 20) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                if let detail {
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 20)
            control()
        }
        .frame(minHeight: detail == nil ? 34 : 46)
        .padding(.horizontal, 2)
    }

    private var advancedOptionDivider: some View {
        Divider()
            .padding(.vertical, 5)
    }

    @ViewBuilder
    private var issueArea: some View {
        let issues = providerIssuePresentations(for: provider)
        if !issues.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(issues.enumerated()), id: \.offset) { _, issue in
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Label(issue.title, systemImage: "exclamationmark.triangle.fill")
                            .font(.callout.weight(.medium))
                            .foregroundStyle(.orange)
                        Text(issue.detail)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                        if let actionTitle = issue.actionTitle {
                            Button(actionTitle) {
                                handleIssueAction(issue.action)
                            }
                            .controlSize(.small)
                        }
                    }
                }
            }
            .padding(10)
            .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
            .padding(.horizontal, 20)
            .padding(.bottom, 10)
        }
    }

    @ViewBuilder
    private var conflictArea: some View {
        if formState.selectedProviderHasExternalChange
            || benchmarkModel.selectedProviderHasExternalChange {
            HStack(spacing: 10) {
                Label("已保存配置在窗口外发生变化", systemImage: "arrow.triangle.2.circlepath")
                    .font(.callout.weight(.medium))
                    .foregroundStyle(.orange)
                Text("请核对当前草稿，再决定保留或重新载入。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("保留当前草稿") {
                    formState.acknowledgeSelectedExternalChange()
                    benchmarkModel.acknowledgeSelectedExternalChange()
                }
                Button("重新载入已保存") {
                    formState.discardSelectedDraft()
                    benchmarkModel.discardSelectionDraft(for: provider.id)
                }
            }
            .padding(10)
            .background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
            .padding(.horizontal, 20)
            .padding(.bottom, 10)
        }
    }

    private var connectionSummary: String {
        formState.connectionDirty ? "连接设置已修改" : "已保存"
    }

    private var operationsBusy: Bool {
        isBusy || benchmarkModel.isBusy
    }

    private var canApplyChanges: Bool {
        !isBusy && !benchmarkModel.isBusy
            && (formState.connectionDirty || benchmarkModel.selectedProviderDirty
                || formState.applyRetryRequired)
            && !formState.selectedProviderHasExternalChange
            && !benchmarkModel.selectedProviderHasExternalChange
    }

    private var applySummary: String {
        var changes: [String] = []
        if formState.connectionDirty {
            changes.append("连接设置已修改")
        }
        if formState.applyRetryRequired {
            changes.append("配置已保存，需重试应用")
        }
        if formState.selectedProviderHasExternalChange
            || benchmarkModel.selectedProviderHasExternalChange {
            changes.append("外部配置已变化，请先核对")
        }
        let counts = benchmarkModel.selectionChangeCounts(for: provider.id)
        if counts.added > 0 { changes.append("加入 \(counts.added) 个") }
        if counts.removed > 0 { changes.append("移除 \(counts.removed) 个") }
        return changes.isEmpty ? "没有待应用的更改" : changes.joined(separator: "；")
    }

    private func handleIssueAction(_ action: ProviderIssueAction) {
        switch action {
        case .connection:
            selectedSection = .connection
        case .models:
            selectedSection = .models
        case .toggle:
            onToggle()
        }
    }

    private var isCustom: Bool { provider.presetID == "custom" }
    private var isAWSBedrock: Bool { provider.presetID == "aws-bedrock" }
    private var isBaiduOneAPI: Bool { provider.presetID == "baidu-oneapi" }
    private var isOpenCodeGo: Bool { requiresOpenCodeGoQuotaCredentials(provider.presetID ?? "") }
    private var apiKeyConfigured: Bool { provider.apiKeyConfigured && !operationsBusy }
    private var quotaCookieConfigured: Bool {
        provider.quotaAuthCookieConfigured == true && !operationsBusy
    }

    private var toggleTitle: String {
        provider.enabled ? "停用" : "启用"
    }

    private var apiKeyPrompt: String {
        provider.apiKeyConfigured ? "已配置；留空保留" : "尚未配置；启用前必须填写"
    }

    private var awsAccessKeyPrompt: String {
        provider.awsSigV4Configured == true ? "Access Key ID 已配置；留空保留" : "Access Key ID"
    }

    private var awsSecretKeyPrompt: String {
        provider.awsSigV4Configured == true
            ? "Secret Access Key 已配置；留空保留"
            : "Secret Access Key"
    }

    private var awsSessionTokenPrompt: String {
        if formState.clearAwsSessionToken {
            return "保存后清除"
        }
        return provider.awsSessionTokenConfigured == true
            ? "Session Token 已配置；留空保留"
            : "Session Token（可选）"
    }

    private var authCookiePrompt: String {
        provider.quotaAuthCookieConfigured == true ? "已配置；留空保留" : "opencode.ai auth cookie"
    }

}

private struct ProviderActionBar: View {
    let isBusy: Bool
    let toggleTitle: String
    let canModify: Bool
    let hasPendingChanges: Bool
    let canApplyChanges: Bool
    let applySummary: String
    let onToggle: () -> Void
    let onTest: () -> Void
    let onApplyChanges: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(spacing: 8) {
                if isBusy {
                    ProgressView()
                        .controlSize(.small)
                }
                Text(applySummary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer()
                Menu {
                    Button("检查模型接口", action: onTest)
                    Button(toggleTitle, action: onToggle)
                } label: {
                    Image(systemName: "ellipsis.circle")
                }
                .menuStyle(.borderlessButton)
                .help("更多服务商操作")
                .accessibilityLabel("更多服务商操作")
                .disabled(!canModify)
                if hasPendingChanges {
                    Button(action: onApplyChanges) {
                        Label("应用更改", systemImage: "square.and.arrow.down")
                    }
                    .keyboardShortcut("s", modifiers: .command)
                    .liquidGlassProminentButton()
                    .disabled(!canApplyChanges)
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
            .background(Color(nsColor: .windowBackgroundColor))
        }
    }
}

private struct ProviderDetailHeader: View {
    let provider: ProviderView

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            ProviderLogo(provider: provider, size: 36)

            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(provider.displayName)
                        .font(.title3.weight(.semibold))
                        .lineLimit(1)
                    ProviderStateBadge(provider: provider)
                }
                HStack(spacing: 5) {
                    Text(provider.kind == .official ? "官方服务商 · 只读" : provider.id)
                        .font(.caption.monospaced())
                    Text("· 已加入 \(provider.selectedModels.count) / 可选 \(provider.cachedModels.count)")
                        .font(.caption)
                }
                .foregroundStyle(.secondary)
                .lineLimit(1)
            }

        }
    }
}

private struct ProviderLogo: View {
    let provider: ProviderView
    let size: CGFloat

    var body: some View {
        Group {
            if let image = providerLogoImageForSettings(provider) {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .padding(size * 0.18)
            } else {
                Image(systemName: "server.rack")
                    .font(.system(size: size * 0.42, weight: .medium))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: size, height: size)
        .background(.quaternary.opacity(0.45), in: RoundedRectangle(cornerRadius: size * 0.22, style: .continuous))
    }
}

private func providerLogoImageForSettings(_ provider: ProviderView) -> NSImage? {
    if let cached = cachedProviderLogoImage(providerID: provider.id, websiteURL: provider.websiteURL) {
        return cached
    }

    let normalized = (provider.presetID ?? provider.id).lowercased()
    let assetName: String
    if normalized.contains("baidu") {
        assetName = "baidu"
    } else if normalized.contains("deepseek") {
        assetName = "deepseek"
    } else if normalized.contains("opencode") {
        assetName = "opencode"
    } else if normalized.contains("openrouter") {
        assetName = "openrouter"
    } else if normalized.contains("aws") || normalized.contains("bedrock") {
        assetName = "aws"
    } else if normalized.contains("openai") || normalized.contains("chatgpt") || provider.kind == .official {
        assetName = "openai"
    } else {
        assetName = "custom"
    }

    let directories = [
        Bundle.main.resourceURL?.appendingPathComponent("ProviderLogos", isDirectory: true),
        Bundle.main.bundleURL.appendingPathComponent("ProviderLogos", isDirectory: true),
    ]
    for directory in directories.compactMap({ $0 }) {
        if let image = NSImage(contentsOf: directory.appendingPathComponent("\(assetName).svg")) {
            return image
        }
    }
    return nil
}

private func providerStatusColor(_ provider: ProviderView) -> Color {
    guard provider.kind == .official || provider.enabled else {
        return .secondary
    }
    switch provider.readiness {
    case "healthy": return .green
    case "degraded": return .orange
    default: return .secondary
    }
}

private struct ProviderStateBadge: View {
    let provider: ProviderView

    private var label: String {
        if provider.kind == .official {
            return "官方"
        }
        if !provider.enabled { return "已停用" }
        return provider.readiness == "healthy" ? "配置就绪" : "需要处理"
    }

    private var color: Color {
        if !provider.enabled && provider.kind != .official {
            return .secondary
        }
        switch provider.readiness {
        case "healthy": return .green
        case "degraded": return .orange
        case "disabled": return .secondary
        default: return .secondary
        }
    }

    var body: some View {
        Text(label)
            .font(.caption.weight(.medium))
            .foregroundStyle(color)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(color.opacity(0.12), in: Capsule())
    }
}
